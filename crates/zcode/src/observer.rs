//! Read-only ZCode dual-sqlite observer.
//! Ported from packages/host/src/backends/zcode/SqliteObserver.ts

use crate::mapper::{map_zcode_row, ZcodeRow};
use agent_deck_protocol::SessionSnapshot;
use rusqlite::{Connection, OpenFlags};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;

const STALE_WINDOW_SECS: i64 = 30 * 60;

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
        let home = dirs_home();
        Self {
            tasks_db_path: home.join(".zcode/v2/tasks-index.sqlite"),
            tool_db_path: home.join(".zcode/cli/db/db.sqlite"),
            exclude_workspaces: vec![],
            exclude_task_ids: vec![],
            fail_on_missing: false,
        }
    }
}

fn dirs_home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
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

    pub fn poll_once(&mut self) -> Result<Vec<SessionSnapshot>, ObserverError> {
        let Some(conn) = self.conn.as_ref() else {
            return Ok(self.last_snapshots.clone());
        };

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
  ) AS detail
FROM tasks t
WHERE t.task_status IN ('running', 'completed', 'error')
  AND t.deleted = 0
  AND t.archived = 0
ORDER BY t.updated_at DESC
LIMIT 20
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

        let signature = signature_of(&snapshots);
        if signature != self.last_signature {
            self.last_signature = signature;
            self.last_snapshots = snapshots.clone();
        } else {
            self.last_snapshots = snapshots.clone();
        }
        Ok(snapshots)
    }

    pub fn last_snapshots(&self) -> &[SessionSnapshot] {
        &self.last_snapshots
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
