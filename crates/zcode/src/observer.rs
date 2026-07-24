//! Read-only ZCode dual-sqlite observer.
//! Ported from packages/host/src/backends/zcode/SqliteObserver.ts

use crate::mapper::{map_zcode_row, ZcodeRow};
use agent_deck_protocol::{home_dir, SessionSnapshot};
use rusqlite::{Connection, OpenFlags};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;

const STALE_WINDOW_SECS: i64 = 30 * 60;

/// How recent a `tool_usage` row must be to count as "session is active right
/// now". ZCode does NOT always write back `completed_at` when a tool finishes
/// (ZCode 3.4.2), leaving zombie rows with `status='running'` and
/// `completed_at IS NULL` forever. Those stale rows (often tens of minutes
/// old) would otherwise pin a finished session to Working forever. A 5-minute
/// window filters them out while still catching any tool that is genuinely
/// running — real tool calls finish well inside 5 minutes.
const ACTIVE_WINDOW_SECS: i64 = 5 * 60;

/// How recently a session must have produced ANY tool_usage row (regardless of
/// status) to count as "still in an active conversation". This is the key to
/// avoiding flicker: between two tool calls there is a gap where no row has
/// `status='running' AND completed_at IS NULL`, so the narrower ACTIVE check
/// flips to Done. But the conversation is plainly not finished — the agent is
/// just thinking/reading between actions. If any tool_usage landed in the last
/// few minutes, the session is alive and should stay Working. Tuned wider than
/// typical inter-tool gaps (seconds) but narrower than how long a user leaves a
/// finished session before starting a new one.
const RECENT_ACTIVITY_SECS: i64 = 3 * 60;

#[derive(Debug, Error)]
pub enum ObserverError {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("db missing: tasks={tasks} tool={tool}")]
    MissingDb { tasks: bool, tool: bool },
}

#[derive(Debug, Clone)]
pub struct SqliteObserverOptions {
    pub tasks_db_path: PathBuf,
    pub tool_db_path: PathBuf,
    pub exclude_workspaces: Vec<String>,
    pub exclude_task_ids: Vec<String>,
    pub fail_on_missing: bool,
}

impl Default for SqliteObserverOptions {
    fn default() -> Self {
        let home = home_dir();
        Self {
            tasks_db_path: home.join(".zcode/v2/tasks-index.sqlite"),
            tool_db_path: home.join(".zcode/cli/db/db.sqlite"),
            exclude_workspaces: vec![],
            exclude_task_ids: vec![],
            fail_on_missing: false,
        }
    }
}

pub struct SqliteObserver {
    opts: SqliteObserverOptions,
    conn: Option<Connection>,
    last_signature: String,
    last_snapshots: Vec<SessionSnapshot>,
}

impl SqliteObserver {
    pub fn new(opts: SqliteObserverOptions) -> Self {
        Self {
            opts,
            conn: None,
            last_signature: String::new(),
            last_snapshots: vec![],
        }
    }

    pub fn open(&mut self) -> Result<(), ObserverError> {
        let tasks_exists = self.opts.tasks_db_path.exists();
        let tool_exists = self.opts.tool_db_path.exists();
        if !tasks_exists || !tool_exists {
            if self.opts.fail_on_missing {
                return Err(ObserverError::MissingDb {
                    tasks: tasks_exists,
                    tool: tool_exists,
                });
            }
            return Ok(());
        }

        let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let conn = Connection::open_with_flags(&self.opts.tasks_db_path, flags)?;
        conn.pragma_update(None, "query_only", true)?;
        // Parameterized ATTACH via format after path validation (path from config, not user text)
        let tool_path = path_for_sql(&self.opts.tool_db_path);
        conn.execute(&format!("ATTACH DATABASE '{tool_path}' AS cli"), [])?;
        self.conn = Some(conn);
        Ok(())
    }

    /// Board poll: recent active-ish tasks only (keeps LED path cheap).
    pub fn poll_once(&mut self) -> Result<Vec<SessionSnapshot>, ObserverError> {
        if self.conn.is_none() {
            return Ok(self.last_snapshots.clone());
        }
        let snapshots =
            self.query_tasks("t.task_status IN ('running', 'completed', 'error')", 20)?;
        let signature = signature_of(&snapshots);
        if signature != self.last_signature {
            self.last_signature = signature;
        }
        self.last_snapshots = snapshots.clone();
        Ok(snapshots)
    }

    /// Latest state of specific tasks by id — no status filter, no LIMIT.
    /// Keeps manually pinned sessions live even when they fall outside the
    /// `poll()` recent-20 window. Ids not in this backend's table are simply
    /// not returned (caller may pass ids owned by other backends).
    pub fn poll_pinned_once(
        &mut self,
        ids: &[String],
    ) -> Result<Vec<SessionSnapshot>, ObserverError> {
        if ids.is_empty() {
            return Ok(vec![]);
        }
        if self.conn.is_none() {
            // Lazy open so pinned refresh works even if open() was skipped.
            let _ = self.open();
        }
        if self.conn.is_none() {
            return Ok(vec![]);
        }
        self.query_tasks_by_ids(ids)
    }

    /// Full catalog for bind picker: all non-deleted/non-archived tasks across
    /// every project, including older history beyond the board poll window.
    pub fn catalog_once(&mut self) -> Result<Vec<SessionSnapshot>, ObserverError> {
        if self.conn.is_none() {
            // Lazy open so bind UI works even if open() was skipped earlier.
            let _ = self.open();
        }
        if self.conn.is_none() {
            return Ok(vec![]);
        }
        // No status filter — any live task row is bindable history.
        self.query_tasks("1=1", 500)
    }

    pub fn last_snapshots(&self) -> &[SessionSnapshot] {
        &self.last_snapshots
    }

    fn query_tasks(
        &self,
        status_predicate: &str,
        limit: usize,
    ) -> Result<Vec<SessionSnapshot>, ObserverError> {
        let conn = self
            .conn
            .as_ref()
            .expect("query_tasks requires an open connection");
        let sql = format!(
            r#"
SELECT
  t.task_id,
  t.title,
  t.task_status,
  t.workspace_path,
  t.updated_at,
  CASE WHEN EXISTS(
    SELECT 1 FROM cli.tool_usage tu
    WHERE tu.session_id = t.task_id
      AND tu.approval_status = 'requested'
      AND tu.status = 'running'
      AND tu.completed_at IS NULL
      AND tu.started_at > (strftime('%s','now') - {STALE_WINDOW_SECS}) * 1000
  ) THEN 1 ELSE 0 END AS waiting,
  (
    SELECT tu.tool_name || ': ' || COALESCE(tu.side_effect_scope, '')
    FROM cli.tool_usage tu
    WHERE tu.session_id = t.task_id
      AND tu.approval_status = 'requested'
      AND tu.status = 'running'
      AND tu.completed_at IS NULL
    ORDER BY tu.started_at DESC
    LIMIT 1
  ) AS detail,
  CASE WHEN EXISTS(
    SELECT 1 FROM cli.tool_usage tu
    WHERE tu.session_id = t.task_id
      AND tu.status = 'running'
      AND tu.completed_at IS NULL
      AND tu.started_at > (strftime('%s','now') - {ACTIVE_WINDOW_SECS}) * 1000
  ) OR EXISTS(
    -- Conversation is alive even between tool calls: the agent thinks/reads
    -- in the gap, so no row is "running" right now, yet activity landed
    -- moments ago. Keep it Working until the session truly goes quiet.
    SELECT 1 FROM cli.tool_usage tu
    WHERE tu.session_id = t.task_id
      AND tu.started_at > (strftime('%s','now') - {RECENT_ACTIVITY_SECS}) * 1000
  ) THEN 1 ELSE 0 END AS active
FROM tasks t
WHERE {status_predicate}
  AND t.deleted = 0
  AND t.archived = 0
ORDER BY t.updated_at DESC
LIMIT {limit}
"#
        );

        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map([], |row| {
            Ok(ZcodeRow {
                task_id: row.get(0)?,
                title: row.get(1)?,
                task_status: row.get(2)?,
                workspace_path: row.get(3)?,
                updated_at: row.get::<_, i64>(4).unwrap_or(0).max(0) as u64,
                waiting: row.get(5)?,
                detail: row.get(6)?,
                active: row.get::<_, i64>(7)? != 0,
            })
        })?;

        let mut snapshots = Vec::new();
        for row in rows {
            let row = row?;
            let snap = map_zcode_row(&row);
            if self.is_excluded(&snap) {
                continue;
            }
            snapshots.push(snap);
        }
        Ok(snapshots)
    }

    /// Same projection as `query_tasks`, but selects by task_id and has no
    /// LIMIT / status filter. Used to refresh pinned sessions regardless of
    /// their age or status.
    fn query_tasks_by_ids(&self, ids: &[String]) -> Result<Vec<SessionSnapshot>, ObserverError> {
        let conn = self
            .conn
            .as_ref()
            .expect("query_tasks_by_ids requires an open connection");
        // Build `IN (?, ?, …)` with one placeholder per id (parameterized →
        // no injection even though ids originate from the local pins file).
        let placeholders: Vec<&str> = std::iter::repeat_n("?", ids.len()).collect();
        let in_clause = placeholders.join(", ");
        let sql = format!(
            r#"
SELECT
  t.task_id,
  t.title,
  t.task_status,
  t.workspace_path,
  t.updated_at,
  CASE WHEN EXISTS(
    SELECT 1 FROM cli.tool_usage tu
    WHERE tu.session_id = t.task_id
      AND tu.approval_status = 'requested'
      AND tu.status = 'running'
      AND tu.completed_at IS NULL
      AND tu.started_at > (strftime('%s','now') - {STALE_WINDOW_SECS}) * 1000
  ) THEN 1 ELSE 0 END AS waiting,
  (
    SELECT tu.tool_name || ': ' || COALESCE(tu.side_effect_scope, '')
    FROM cli.tool_usage tu
    WHERE tu.session_id = t.task_id
      AND tu.approval_status = 'requested'
      AND tu.status = 'running'
      AND tu.completed_at IS NULL
    ORDER BY tu.started_at DESC
    LIMIT 1
  ) AS detail,
  CASE WHEN EXISTS(
    SELECT 1 FROM cli.tool_usage tu
    WHERE tu.session_id = t.task_id
      AND tu.status = 'running'
      AND tu.completed_at IS NULL
      AND tu.started_at > (strftime('%s','now') - {ACTIVE_WINDOW_SECS}) * 1000
  ) OR EXISTS(
    SELECT 1 FROM cli.tool_usage tu
    WHERE tu.session_id = t.task_id
      AND tu.started_at > (strftime('%s','now') - {RECENT_ACTIVITY_SECS}) * 1000
  ) THEN 1 ELSE 0 END AS active
FROM tasks t
WHERE t.task_id IN ({in_clause})
  AND t.deleted = 0
  AND t.archived = 0
"#
        );

        let params: Vec<&dyn rusqlite::ToSql> =
            ids.iter().map(|id| id as &dyn rusqlite::ToSql).collect();
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params.as_slice(), |row| {
            Ok(ZcodeRow {
                task_id: row.get(0)?,
                title: row.get(1)?,
                task_status: row.get(2)?,
                workspace_path: row.get(3)?,
                updated_at: row.get::<_, i64>(4).unwrap_or(0).max(0) as u64,
                waiting: row.get(5)?,
                detail: row.get(6)?,
                active: row.get::<_, i64>(7)? != 0,
            })
        })?;

        let mut snapshots = Vec::new();
        for row in rows {
            let row = row?;
            let snap = map_zcode_row(&row);
            if self.is_excluded(&snap) {
                continue;
            }
            snapshots.push(snap);
        }
        Ok(snapshots)
    }

    fn is_excluded(&self, snap: &SessionSnapshot) -> bool {
        if self
            .opts
            .exclude_task_ids
            .iter()
            .any(|id| id == &snap.session_id)
        {
            return true;
        }
        if let Some(ref wp) = snap.workspace_path {
            for ex in &self.opts.exclude_workspaces {
                if !ex.is_empty() && wp.contains(ex) {
                    return true;
                }
            }
        }
        false
    }
}

fn path_for_sql(path: &Path) -> String {
    path.to_string_lossy().replace('\'', "''")
}

fn signature_of(snapshots: &[SessionSnapshot]) -> String {
    snapshots
        .iter()
        .map(|s| {
            format!(
                "{}|{:?}|{:?}|{:?}|{:?}|{}|{}",
                s.session_id, s.status, s.risk, s.detail, s.waiting_since, s.updated_at, s.title
            )
        })
        .collect::<Vec<_>>()
        .join(";")
}

#[allow(dead_code)]
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
