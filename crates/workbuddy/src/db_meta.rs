//! Read-only overlay from WorkBuddy's local sqlite (`~/.workbuddy/workbuddy.db`).
//!
//! The jsonl event stream is **not** the source of truth for renames: when a user
//! edits a session title in the WorkBuddy UI, only `sessions.custom_title` is
//! updated. jsonl may still carry the old `ai-title` / never get a `custom-title`
//! event. Verified 2026-07-23:
//!
//! ```text
//! sessions.id            = 0ad316a2-...
//! sessions.title         = 打招呼          -- original AI title
//! sessions.custom_title  = ai              -- user rename (not in jsonl)
//! ```
//!
//! This module also classifies bind-picker groups:
//! - `is_playground` → 任务 (ad-hoc timestamp folders under ~/WorkBuddy)
//! - `is_background_automation` / `automation-*` cwd → 自动化
//! - otherwise → 项目
//!
//! Automation display names come from the `automations` table (`name` + `cwds`).

use rusqlite::{Connection, OpenFlags};
use std::collections::HashMap;
use std::path::Path;

/// Per-session metadata from `workbuddy.db`.
#[derive(Debug, Clone, Default)]
pub struct SessionMeta {
    /// Model/original title (`sessions.title`).
    pub title: Option<String>,
    /// User-set title (`sessions.custom_title`) — wins over everything.
    pub custom_title: Option<String>,
    /// Workspace path from DB (`sessions.cwd`).
    pub cwd: Option<String>,
    /// WorkBuddy "任务" playground sessions (no real project folder).
    pub is_playground: bool,
    /// Headless / scheduled automation runs.
    pub is_background_automation: bool,
    /// `sessions.status` — `archived` sessions must be hidden from the bind
    /// picker (they still exist on disk with jsonl + non-null row).
    pub status: Option<String>,
    /// `sessions.updated_at` (ms epoch) — used to rank tasks/automations by
    /// recency so the bind picker mirrors WorkBuddy's UI ordering.
    pub updated_at: Option<u64>,
}

/// True when the DB row marks this session as archived. WorkBuddy uses
/// `sessions.status = 'archived'` (case-insensitive) independent of
/// `deleted_at` — both must be checked.
pub fn is_archived(meta: &SessionMeta) -> bool {
    meta.status
        .as_deref()
        .map(|s| s.eq_ignore_ascii_case("archived"))
        .unwrap_or(false)
}

/// Preferred display title: user rename > model title.
pub fn preferred_title(meta: &SessionMeta) -> Option<String> {
    meta.custom_title
        .as_ref()
        .filter(|s| !s.is_empty())
        .cloned()
        .or_else(|| meta.title.as_ref().filter(|s| !s.is_empty()).cloned())
}

/// Load all live (non-deleted) sessions. Missing/unreadable DB → empty map
/// (never errors — jsonl remains the fallback).
pub fn load_session_meta(db_path: &Path) -> HashMap<String, SessionMeta> {
    if !db_path.is_file() {
        return HashMap::new();
    }
    let conn = match Connection::open_with_flags(
        db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ) {
        Ok(c) => c,
        Err(_) => return HashMap::new(),
    };
    // Prefer full query with automation flag; fall back if schema is older.
    let mut out = load_sessions_full(&conn);
    if out.is_empty() {
        out = load_sessions_basic(&conn);
    }
    out
}

fn load_sessions_full(conn: &Connection) -> HashMap<String, SessionMeta> {
    let mut stmt = match conn.prepare(
        "SELECT id, title, custom_title, cwd,
                COALESCE(is_playground, 0),
                COALESCE(is_background_automation, 0),
                status, updated_at
         FROM sessions
         WHERE deleted_at IS NULL",
    ) {
        Ok(s) => s,
        Err(_) => return HashMap::new(),
    };
    let rows = stmt.query_map([], |row| {
        let id: String = row.get(0)?;
        let title: Option<String> = row.get(1)?;
        let custom_title: Option<String> = row.get(2)?;
        let cwd: Option<String> = row.get(3)?;
        let is_playground: i64 = row.get(4)?;
        let is_auto: i64 = row.get(5)?;
        let status: Option<String> = row.get(6)?;
        let updated_at: Option<i64> = row.get(7)?;
        Ok((
            id,
            SessionMeta {
                title: title.filter(|s| !s.is_empty()),
                custom_title: custom_title.filter(|s| !s.is_empty()),
                cwd: cwd.filter(|s| !s.is_empty()),
                is_playground: is_playground != 0,
                is_background_automation: is_auto != 0,
                status: status.filter(|s| !s.is_empty()),
                updated_at: updated_at.map(|v| v.max(0) as u64),
            },
        ))
    });
    collect_rows(rows)
}

fn load_sessions_basic(conn: &Connection) -> HashMap<String, SessionMeta> {
    // Oldest schemas lack is_background_automation / status / updated_at.
    let mut stmt = match conn.prepare(
        "SELECT id, title, custom_title, cwd, COALESCE(is_playground, 0), status, updated_at
         FROM sessions
         WHERE deleted_at IS NULL",
    ) {
        Ok(s) => s,
        Err(_) => match conn.prepare(
            "SELECT id, title, custom_title, cwd, COALESCE(is_playground, 0)
             FROM sessions
             WHERE deleted_at IS NULL",
        ) {
            Ok(s) => s,
            Err(_) => return HashMap::new(),
        },
    };
    let rows = stmt.query_map([], |row| {
        let id: String = row.get(0)?;
        let title: Option<String> = row.get(1)?;
        let custom_title: Option<String> = row.get(2)?;
        let cwd: Option<String> = row.get(3)?;
        let is_playground: i64 = row.get(4)?;
        let status: Option<String> = row.get(5).ok().flatten();
        let updated_at: Option<i64> = row.get(6).ok().flatten();
        Ok((
            id,
            SessionMeta {
                title: title.filter(|s| !s.is_empty()),
                custom_title: custom_title.filter(|s| !s.is_empty()),
                cwd: cwd.filter(|s| !s.is_empty()),
                is_playground: is_playground != 0,
                is_background_automation: false,
                status: status.filter(|s| !s.is_empty()),
                updated_at: updated_at.map(|v| v.max(0) as u64),
            },
        ))
    });
    collect_rows(rows)
}

fn collect_rows(
    rows: Result<
        rusqlite::MappedRows<'_, impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<(String, SessionMeta)>>,
        rusqlite::Error,
    >,
) -> HashMap<String, SessionMeta> {
    let Ok(rows) = rows else {
        return HashMap::new();
    };
    let mut out = HashMap::new();
    for row in rows.flatten() {
        out.insert(row.0, row.1);
    }
    out
}

/// Session ids with `deleted_at` set. Used to hide soft-deleted sessions whose
/// jsonl files may still exist on disk.
pub fn load_deleted_session_ids(db_path: &Path) -> std::collections::HashSet<String> {
    use std::collections::HashSet;
    if !db_path.is_file() {
        return HashSet::new();
    }
    let conn = match Connection::open_with_flags(
        db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ) {
        Ok(c) => c,
        Err(_) => return HashSet::new(),
    };
    let mut stmt = match conn.prepare(
        "SELECT id FROM sessions WHERE deleted_at IS NOT NULL",
    ) {
        Ok(s) => s,
        Err(_) => return HashSet::new(),
    };
    let rows = stmt.query_map([], |row| row.get::<_, String>(0));
    let Ok(rows) = rows else {
        return HashSet::new();
    };
    rows.flatten().collect()
}

/// Map automation workspace cwd → human name from the `automations` table.
/// Missing/unreadable table → empty map.
pub fn load_automation_names(db_path: &Path) -> HashMap<String, String> {
    if !db_path.is_file() {
        return HashMap::new();
    }
    let conn = match Connection::open_with_flags(
        db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ) {
        Ok(c) => c,
        Err(_) => return HashMap::new(),
    };
    // Prefer non-deleted when column exists; fall back for older schemas.
    let mut stmt = match conn.prepare(
        "SELECT name, cwds FROM automations WHERE deleted_at IS NULL",
    ) {
        Ok(s) => s,
        Err(_) => match conn.prepare("SELECT name, cwds FROM automations") {
            Ok(s) => s,
            Err(_) => return HashMap::new(),
        },
    };
    let rows = stmt.query_map([], |row| {
        let name: String = row.get(0)?;
        let cwds: String = row.get(1)?;
        Ok((name, cwds))
    });
    let Ok(rows) = rows else {
        return HashMap::new();
    };
    let mut out = HashMap::new();
    for (name, cwds_json) in rows.flatten() {
        if name.is_empty() {
            continue;
        }
        // cwds is a JSON array of paths, e.g. ["\/Users\/...\/automation-..."]
        if let Ok(paths) = serde_json::from_str::<Vec<String>>(&cwds_json) {
            for p in paths {
                if !p.is_empty() {
                    out.insert(normalize_path(&p), name.clone());
                }
            }
        }
    }
    out
}

fn normalize_path(p: &str) -> String {
    let mut s = p.replace('\\', "/");
    while s.ends_with('/') && s.len() > 1 {
        s.pop();
    }
    s
}

/// Classify a WorkBuddy workspace for the bind picker.
///
/// Priority matches WorkBuddy's own UI grouping:
/// 1. **自动化**: workspace path leaf starts with `automation-` (WorkBuddy
///    provisions a dedicated `~/WorkBuddy/automation-<ts>` folder per
///    scheduled automation). The `is_background_automation` flag alone is NOT
///    enough — a normal project (e.g. `~/WorkBuddy/Claw`) can have stray
///    sessions with that flag set, so the path shape is authoritative.
/// 2. **任务**: `is_playground=1` or a `~/WorkBuddy/<YYYY-MM-DD-HH-MM-SS>`
///    timestamp folder. Each playground session gets its own throwaway cwd.
/// 3. **项目**: everything else (real folder the user opened).
pub fn classify_workspace(
    cwd: Option<&str>,
    meta: Option<&SessionMeta>,
    automation_names: &HashMap<String, String>,
    session_title: &str,
) -> (agent_deck_protocol::ProjectCategory, String) {
    use agent_deck_protocol::ProjectCategory;

    let cwd = cwd.map(normalize_path);
    let cwd_leaf = cwd.as_deref().map(path_leaf).unwrap_or("");
    // Automation MUST be decided by path shape — the flag is unreliable.
    let is_auto = cwd_leaf.starts_with("automation-");
    let is_task = meta.map(|m| m.is_playground).unwrap_or(false)
        || (is_task_timestamp_folder(cwd_leaf) && is_under_workbuddy_home(cwd.as_deref().unwrap_or("")));

    if is_auto {
        let label = cwd
            .as_deref()
            .and_then(|p| automation_names.get(p).cloned())
            .or_else(|| meta.and_then(preferred_title))
            .filter(|s| !s.is_empty() && s != "(untitled)")
            .or_else(|| {
                let t = session_title.trim();
                if !t.is_empty() && t != "(untitled)" {
                    Some(t.to_string())
                } else {
                    None
                }
            })
            .unwrap_or_else(|| {
                cwd.as_deref()
                    .map(|p| format!("自动化 · {}", path_leaf(p)))
                    .unwrap_or_else(|| "自动化".into())
            });
        return (ProjectCategory::Automation, label);
    }

    if is_task {
        // Task folders are ephemeral; show the session title as the group label.
        let label = meta
            .and_then(preferred_title)
            .filter(|s| !s.is_empty() && s != "(untitled)")
            .or_else(|| {
                let t = session_title.trim();
                if !t.is_empty() && t != "(untitled)" {
                    Some(t.to_string())
                } else {
                    None
                }
            })
            .unwrap_or_else(|| {
                cwd.as_deref()
                    .map(|p| format!("任务 · {}", path_leaf(p)))
                    .unwrap_or_else(|| "任务".into())
            });
        return (ProjectCategory::Task, label);
    }

    // Real project: folder leaf name.
    let label = cwd
        .as_deref()
        .map(|p| path_leaf(p).to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "(unknown project)".into());
    (ProjectCategory::Project, label)
}

fn path_leaf(path: &str) -> &str {
    path.rsplit('/').find(|s| !s.is_empty()).unwrap_or(path)
}

fn is_under_workbuddy_home(path: &str) -> bool {
    // .../WorkBuddy/<leaf> or .../workbuddy/<leaf>
    let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if parts.len() < 2 {
        return false;
    }
    let parent = parts[parts.len() - 2];
    parent.eq_ignore_ascii_case("WorkBuddy")
}

/// Detects the Claw workspace — WorkBuddy's built-in assistant space.
///
/// Claw (`~/WorkBuddy/Claw`) has its own dedicated entry in the WorkBuddy
/// sidebar, so its sessions are *anchored* out of the normal 任务/空间 lists
/// (`projectLatestClawSessionAnchor` in the renderer). We mirror that by hiding
/// Claw sessions from the bind picker. Detection mirrors
/// `isClawWorkspaceCwd`: leaf is "Claw" (case-insensitive) under the
/// WorkBuddy workspace root.
pub fn is_claw_workspace(cwd: Option<&str>) -> bool {
    let Some(cwd) = cwd else { return false };
    let normalized = normalize_path(cwd);
    let parts: Vec<&str> = normalized.split('/').filter(|s| !s.is_empty()).collect();
    if parts.len() < 2 {
        return false;
    }
    let leaf = parts[parts.len() - 1];
    let parent = parts[parts.len() - 2];
    leaf.eq_ignore_ascii_case("Claw") && parent.eq_ignore_ascii_case("WorkBuddy")
}

/// Timestamp / compact-date task folders created by WorkBuddy for ad-hoc tasks.
fn is_task_timestamp_folder(leaf: &str) -> bool {
    // 2026-07-23-16-37-26
    if leaf.len() == 19
        && leaf.as_bytes().get(4) == Some(&b'-')
        && leaf.as_bytes().get(7) == Some(&b'-')
        && leaf.as_bytes().get(10) == Some(&b'-')
    {
        return leaf.chars().all(|c| c.is_ascii_digit() || c == '-');
    }
    // 20260322151629 (14 digits)
    leaf.len() == 14 && leaf.chars().all(|c| c.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_deck_protocol::ProjectCategory;
    use rusqlite::Connection;
    use tempfile::tempdir;

    fn make_db(path: &Path) {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(
            "CREATE TABLE sessions (
                id TEXT PRIMARY KEY,
                title TEXT,
                custom_title TEXT,
                cwd TEXT,
                is_playground INTEGER,
                is_background_automation INTEGER,
                status TEXT,
                updated_at INTEGER,
                deleted_at INTEGER
            );
            CREATE TABLE automations (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                prompt TEXT NOT NULL DEFAULT '',
                status TEXT NOT NULL DEFAULT 'ACTIVE',
                cwds TEXT NOT NULL DEFAULT '[]',
                deleted_at INTEGER
            );",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sessions (id, title, custom_title, cwd, is_playground, is_background_automation, status, updated_at, deleted_at)
             VALUES ('s1', '打招呼', 'ai 趋势', '/Users/x/WorkBuddy/2026-07-23-16-37-26', 1, 0, 'completed', 100, NULL)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sessions (id, title, custom_title, cwd, is_playground, is_background_automation, status, updated_at, deleted_at)
             VALUES ('s2', 'only-ai', NULL, '/tmp/p', 0, 0, 'completed', 200, NULL)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sessions (id, title, custom_title, cwd, is_playground, is_background_automation, status, updated_at, deleted_at)
             VALUES ('auto1', NULL, 'nvidia', '/Users/x/WorkBuddy/automation-2026-07-17-10-27-12', 0, 1, 'completed', 300, NULL)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sessions (id, title, custom_title, cwd, is_playground, is_background_automation, status, updated_at, deleted_at)
             VALUES ('gone', 'deleted', 'x', '/tmp', 0, 0, 'completed', 1, 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO automations (id, name, cwds, deleted_at)
             VALUES ('a1', 'nvidia 日报', '[\"/Users/x/WorkBuddy/automation-2026-07-17-10-27-12\"]', NULL)",
            [],
        )
        .unwrap();
    }

    #[test]
    fn loads_custom_title_over_title() {
        let dir = tempdir().unwrap();
        let db = dir.path().join("workbuddy.db");
        make_db(&db);
        let map = load_session_meta(&db);
        assert_eq!(map.len(), 3); // deleted filtered
        assert_eq!(preferred_title(&map["s1"]).as_deref(), Some("ai 趋势"));
        assert_eq!(preferred_title(&map["s2"]).as_deref(), Some("only-ai"));
        assert!(map["s1"].is_playground);
        assert!(!map["s2"].is_playground);
        assert!(map["auto1"].is_background_automation);
    }

    #[test]
    fn missing_db_is_empty() {
        let map = load_session_meta(Path::new("/no/such/workbuddy.db"));
        assert!(map.is_empty());
    }

    #[test]
    fn automation_names_from_cwds() {
        let dir = tempdir().unwrap();
        let db = dir.path().join("workbuddy.db");
        make_db(&db);
        let names = load_automation_names(&db);
        assert_eq!(
            names
                .get("/Users/x/WorkBuddy/automation-2026-07-17-10-27-12")
                .map(String::as_str),
            Some("nvidia 日报")
        );
    }

    #[test]
    fn classify_task_uses_title() {
        let meta = SessionMeta {
            title: Some("打招呼".into()),
            custom_title: Some("ai 趋势".into()),
            cwd: Some("/Users/x/WorkBuddy/2026-07-23-16-37-26".into()),
            is_playground: true,
            is_background_automation: false,
            ..Default::default()
        };
        let (cat, label) = classify_workspace(
            meta.cwd.as_deref(),
            Some(&meta),
            &HashMap::new(),
            "打招呼",
        );
        assert_eq!(cat, ProjectCategory::Task);
        assert_eq!(label, "ai 趋势");
    }

    #[test]
    fn classify_automation_prefers_table_name() {
        let mut names = HashMap::new();
        names.insert(
            "/Users/x/WorkBuddy/automation-2026-07-17-10-27-12".into(),
            "nvidia 日报".into(),
        );
        let meta = SessionMeta {
            title: None,
            custom_title: Some("nvidia".into()),
            cwd: Some("/Users/x/WorkBuddy/automation-2026-07-17-10-27-12".into()),
            is_playground: false,
            is_background_automation: true,
            ..Default::default()
        };
        let (cat, label) = classify_workspace(
            meta.cwd.as_deref(),
            Some(&meta),
            &names,
            "(untitled)",
        );
        assert_eq!(cat, ProjectCategory::Automation);
        assert_eq!(label, "nvidia 日报");
    }

    #[test]
    fn classify_project_uses_folder_leaf() {
        let meta = SessionMeta {
            title: Some("打招呼".into()),
            custom_title: None,
            cwd: Some("/Users/x/WorkBuddy/workbuddy 测试文件夹".into()),
            is_playground: false,
            is_background_automation: false,
            ..Default::default()
        };
        let (cat, label) = classify_workspace(
            meta.cwd.as_deref(),
            Some(&meta),
            &HashMap::new(),
            "打招呼",
        );
        assert_eq!(cat, ProjectCategory::Project);
        assert_eq!(label, "workbuddy 测试文件夹");
    }

    /// A project folder (not automation-*) must classify as 项目 even if some
    /// of its sessions carry the is_background_automation flag. The path shape
    /// is authoritative; the flag alone is unreliable.
    #[test]
    fn classify_project_with_stray_automation_flag() {
        let meta = SessionMeta {
            cwd: Some("/Users/x/WorkBuddy/Claw".into()),
            is_background_automation: true, // stray flag — must NOT make it 自动化
            ..Default::default()
        };
        let (cat, _) = classify_workspace(meta.cwd.as_deref(), Some(&meta), &HashMap::new(), "");
        assert_eq!(cat, ProjectCategory::Project);
    }

    /// Archived sessions (status='archived') must be detected for UI hiding.
    #[test]
    fn is_archived_case_insensitive() {
        let meta = SessionMeta {
            status: Some("Archived".into()),
            ..Default::default()
        };
        assert!(is_archived(&meta));
        let meta2 = SessionMeta {
            status: Some("completed".into()),
            ..Default::default()
        };
        assert!(!is_archived(&meta2));
    }

    /// Claw workspace (`~/WorkBuddy/Claw`) must be detected so it can be hidden
    /// — it has a dedicated WorkBuddy sidebar entry and is anchored out of the
    /// normal 任务/空间 lists.
    #[test]
    fn claw_workspace_detected_case_insensitive() {
        assert!(is_claw_workspace(Some("/Users/x/WorkBuddy/Claw")));
        assert!(is_claw_workspace(Some("/Users/x/workbuddy/claw")));
        // Not under WorkBuddy root.
        assert!(!is_claw_workspace(Some("/Users/x/Desktop/Claw")));
        // Different name.
        assert!(!is_claw_workspace(Some("/Users/x/WorkBuddy/mini-02")));
        assert!(!is_claw_workspace(None));
    }
}
