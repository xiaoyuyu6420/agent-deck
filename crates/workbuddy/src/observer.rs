//! Read-only WorkBuddy jsonl observer + sqlite title overlay.
//!
//! Each WorkBuddy task is a `<session-id>.jsonl` file under
//! `~/.workbuddy/projects/<workspace>/`. This observer scans that tree
//! read-only, decodes the event stream per file, and maps it to
//! `SessionSnapshot`s. Structure mirrors the zcode `SqliteObserver`: an
//! options struct with `~/.workbuddy` defaults, an `open()` that probes the
//! tree exists, and three poll tiers (board / catalog / pinned).
//!
//! **Title source of truth**: user renames live in
//! `~/.workbuddy/workbuddy.db` (`sessions.custom_title`), **not** in jsonl.
//! Each poll reloads DB meta and overlays titles/cwd onto jsonl snapshots.
//!
//! Like zcode, a missing tree degrades to an empty observer (returns
//! `Ok(vec![])`) rather than an error — one unavailable backend must never
//! break the others.

use crate::db_meta::{
    classify_workspace, is_archived, is_claw_workspace, load_automation_names,
    load_deleted_session_ids, load_session_meta, preferred_title, SessionMeta,
};
use crate::mapper::{aggregate, map_session, SessionEvent};
use agent_deck_protocol::SessionSnapshot;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ObserverError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("workbuddy tree missing: {0}")]
    MissingTree(PathBuf),
}

#[derive(Debug, Clone)]
pub struct JsonlObserverOptions {
    /// Root of the per-session jsonl tree, normally `~/.workbuddy/projects`.
    pub projects_dir: PathBuf,
    /// WorkBuddy local sqlite with session titles. Normally
    /// `~/.workbuddy/workbuddy.db`. Optional — missing DB just skips overlay.
    pub db_path: PathBuf,
    pub exclude_workspaces: Vec<String>,
    pub exclude_task_ids: Vec<String>,
    /// Fail (instead of degrade) when the tree is absent. Tests use this.
    pub fail_on_missing: bool,
    pub max_sessions: usize,
    pub catalog_max_sessions: usize,
}

impl Default for JsonlObserverOptions {
    fn default() -> Self {
        let home = agent_deck_protocol::home_dir();
        Self {
            projects_dir: home.join(".workbuddy/projects"),
            db_path: home.join(".workbuddy/workbuddy.db"),
            exclude_workspaces: vec![],
            exclude_task_ids: vec![],
            fail_on_missing: false,
            max_sessions: 20,
            catalog_max_sessions: 500,
        }
    }
}

pub struct JsonlObserver {
    opts: JsonlObserverOptions,
    available: bool,
    last_snapshots: Vec<SessionSnapshot>,
}

impl JsonlObserver {
    pub fn new(opts: JsonlObserverOptions) -> Self {
        Self {
            opts,
            available: false,
            last_snapshots: vec![],
        }
    }

    /// Probe that the projects tree exists. Never errors when the tree is
    /// absent (unless `fail_on_missing` is set) — mirrors zcode's graceful
    /// degradation contract.
    pub fn open(&mut self) -> Result<(), ObserverError> {
        if self.opts.projects_dir.is_dir() {
            self.available = true;
            return Ok(());
        }
        if self.opts.fail_on_missing {
            return Err(ObserverError::MissingTree(self.opts.projects_dir.clone()));
        }
        self.available = false;
        Ok(())
    }

    pub fn last_snapshots(&self) -> &[SessionSnapshot] {
        &self.last_snapshots
    }

    /// Board poll: up to `max_sessions` most-recently-touched sessions.
    pub fn poll_once(&mut self) -> Result<Vec<SessionSnapshot>, ObserverError> {
        let now = now_ms();
        let mut snaps = self.scan_all(now)?;
        // Most recently updated first.
        snaps.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        snaps.truncate(self.opts.max_sessions);
        snaps.retain(|s| !self.is_excluded(s));
        self.last_snapshots = snaps.clone();
        Ok(snaps)
    }

    /// Full catalog for the bind picker: wider history, same scan.
    pub fn catalog_once(&mut self) -> Result<Vec<SessionSnapshot>, ObserverError> {
        if !self.available {
            let _ = self.open();
        }
        let now = now_ms();
        let mut snaps = self.scan_all(now)?;
        snaps.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        snaps.truncate(self.opts.catalog_max_sessions);
        snaps.retain(|s| !self.is_excluded(s));
        Ok(snaps)
    }

    /// Latest state of specific sessions by id — no limit, no recency filter.
    /// Keeps pinned sessions live even outside the board poll window.
    pub fn poll_pinned_once(
        &mut self,
        ids: &[String],
    ) -> Result<Vec<SessionSnapshot>, ObserverError> {
        if ids.is_empty() {
            return Ok(vec![]);
        }
        if !self.available {
            let _ = self.open();
        }
        if !self.available {
            return Ok(vec![]);
        }
        let now = now_ms();
        let snaps = self.scan_all(now)?;
        Ok(snaps
            .into_iter()
            .filter(|s| ids.iter().any(|id| id == &s.session_id))
            .collect())
    }

    /// Walk the projects tree, decode every `<session-id>.jsonl`, and map it.
    fn scan_all(&self, now: u64) -> Result<Vec<SessionSnapshot>, ObserverError> {
        if !self.available {
            return Ok(vec![]);
        }
        // Reload titles/classification each poll — renames must show up without restart.
        let db_meta = load_session_meta(&self.opts.db_path);
        let automation_names = load_automation_names(&self.opts.db_path);
        let deleted = load_deleted_session_ids(&self.opts.db_path);
        let mut snaps = Vec::new();
        // Projects tree: <projects_dir>/<workspace-dir>/<session-id>.jsonl
        for ws_entry in fs::read_dir(&self.opts.projects_dir)? {
            let ws_entry = ws_entry?;
            if !ws_entry.file_type()?.is_dir() {
                continue;
            }
            for entry in fs::read_dir(ws_entry.path())? {
                let entry = entry?;
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                    continue;
                }
                let session_id = match path.file_stem().and_then(|s| s.to_str()) {
                    Some(s) => s.to_string(),
                    None => continue,
                };
                // Soft-deleted in sqlite: jsonl may linger — hide from catalog/board.
                if deleted.contains(&session_id) {
                    continue;
                }
                // Archived via sessions.status: WorkBuddy keeps the row + jsonl
                // but hides it from the UI. Mirror that here.
                if let Some(meta) = db_meta.get(&session_id) {
                    if is_archived(meta) {
                        continue;
                    }
                }
                if let Some(mut snap) =
                    self.read_session(&path, &session_id, now, &db_meta, &automation_names)
                {
                    // Claw workspace is anchored out of the normal lists in the
                    // WorkBuddy UI (it has a dedicated sidebar entry). Mirror that
                    // so the bind picker doesn't show a stray "Claw" space.
                    if is_claw_workspace(snap.workspace_path.as_deref()) {
                        continue;
                    }
                    // Prefer DB updated_at for ordering (WorkBuddy UI sorts by it).
                    if let Some(meta) = db_meta.get(&session_id) {
                        if let Some(ts) = meta.updated_at {
                            snap.updated_at = ts;
                        }
                    }
                    snaps.push(snap);
                }
            }
        }
        Ok(snaps)
    }

    /// Read and fold one session file. Returns None on decode failure of the
    /// whole file (malformed sessions are skipped, never propagated).
    fn read_session(
        &self,
        path: &Path,
        session_id: &str,
        now: u64,
        db_meta: &HashMap<String, SessionMeta>,
        automation_names: &HashMap<String, String>,
    ) -> Option<SessionSnapshot> {
        let contents = fs::read_to_string(path).ok()?;
        let mut events: Vec<SessionEvent> = Vec::new();
        for line in contents.lines() {
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<SessionEvent>(line) {
                Ok(ev) => events.push(ev),
                // Skip unparseable lines (forward-compat with format drift).
                Err(_) => continue,
            }
        }
        if events.is_empty() {
            return None;
        }
        // Soft Working (streaming / thinking) needs file mtime: after user
        // Stop, WorkBuddy freezes the incomplete row and stops appending.
        let file_mtime_ms = fs::metadata(path)
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as u64);
        let signals = aggregate(&events, session_id, file_mtime_ms);
        let mut snap = map_session(&signals, now);
        // DB overlay: user renames live only in workbuddy.db.
        let meta = db_meta.get(session_id);
        if let Some(meta) = meta {
            if let Some(t) = preferred_title(meta) {
                snap.title = t;
            }
            if snap.workspace_path.is_none() {
                if let Some(cwd) = meta.cwd.clone() {
                    snap.workspace_path = Some(cwd);
                }
            }
        }
        // Bind-picker classification (任务 / 项目 / 自动化).
        let (cat, label) = classify_workspace(
            snap.workspace_path.as_deref(),
            meta,
            automation_names,
            &snap.title,
        );
        snap.project_category = Some(cat);
        snap.project_label = Some(label);
        Some(snap)
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

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_deck_protocol::{BackendId, ProjectCategory};
    use std::fs;
    use tempfile::tempdir;

    fn write_session(dir: &Path, id: &str, lines: &[&str]) {
        let path = dir.join(format!("{id}.jsonl"));
        fs::write(&path, lines.join("\n")).unwrap();
    }

    #[test]
    fn missing_tree_degrades_to_empty() {
        let tmp = tempdir().unwrap();
        let mut obs = JsonlObserver::new(JsonlObserverOptions {
            projects_dir: tmp.path().join("nope"),
            ..Default::default()
        });
        obs.open().unwrap();
        let snaps = obs.poll_once().unwrap();
        assert!(snaps.is_empty());
    }

    #[test]
    fn missing_tree_fails_when_configured() {
        let tmp = tempdir().unwrap();
        let mut obs = JsonlObserver::new(JsonlObserverOptions {
            projects_dir: tmp.path().join("nope"),
            fail_on_missing: true,
            ..Default::default()
        });
        assert!(obs.open().is_err());
    }

    #[test]
    fn scans_sessions_across_workspaces() {
        let tmp = tempdir().unwrap();
        let projects = tmp.path().join("projects");
        let ws1 = projects.join("ws1");
        let ws2 = projects.join("ws2");
        fs::create_dir_all(&ws1).unwrap();
        fs::create_dir_all(&ws2).unwrap();

        write_session(
            &ws1,
            "s1",
            &[r#"{"timestamp":1000,"type":"ai-title","aiTitle":"task one","cwd":"/a"}"#],
        );
        write_session(
            &ws2,
            "s2",
            &[r#"{"timestamp":2000,"type":"ai-title","aiTitle":"task two","cwd":"/b"}"#],
        );

        let mut obs = JsonlObserver::new(JsonlObserverOptions {
            projects_dir: projects,
            ..Default::default()
        });
        obs.open().unwrap();
        let snaps = obs.poll_once().unwrap();
        assert_eq!(snaps.len(), 2);
        // Most recent first.
        assert_eq!(snaps[0].session_id, "s2");
        assert_eq!(snaps[0].title, "task two");
        assert_eq!(snaps[1].session_id, "s1");
        assert_eq!(snaps[1].title, "task one");
        assert!(snaps.iter().all(|s| s.backend == BackendId::Workbuddy));
    }

    #[test]
    fn ignores_non_jsonl_files() {
        let tmp = tempdir().unwrap();
        let projects = tmp.path().join("projects");
        let ws = projects.join("ws");
        fs::create_dir_all(&ws).unwrap();
        fs::write(ws.join("readme.txt"), "ignore me").unwrap();
        write_session(&ws, "s1", &[r#"{"timestamp":1,"type":"message"}"#]);

        let mut obs = JsonlObserver::new(JsonlObserverOptions {
            projects_dir: projects,
            ..Default::default()
        });
        obs.open().unwrap();
        assert_eq!(obs.poll_once().unwrap().len(), 1);
    }

    #[test]
    fn poll_pinned_filters_by_id() {
        let tmp = tempdir().unwrap();
        let projects = tmp.path().join("projects");
        let ws = projects.join("ws");
        fs::create_dir_all(&ws).unwrap();
        write_session(&ws, "s1", &[r#"{"timestamp":1,"type":"message"}"#]);
        write_session(&ws, "s2", &[r#"{"timestamp":2,"type":"message"}"#]);

        let mut obs = JsonlObserver::new(JsonlObserverOptions {
            projects_dir: projects,
            ..Default::default()
        });
        obs.open().unwrap();
        let pinned = obs.poll_pinned_once(&["s2".into()]).unwrap();
        assert_eq!(pinned.len(), 1);
        assert_eq!(pinned[0].session_id, "s2");
    }

    #[test]
    fn skips_malformed_lines() {
        let tmp = tempdir().unwrap();
        let projects = tmp.path().join("projects");
        let ws = projects.join("ws");
        fs::create_dir_all(&ws).unwrap();
        write_session(
            &ws,
            "s1",
            &[
                "this is not json",
                r#"{"timestamp":500,"type":"ai-title","aiTitle":"ok"}"#,
            ],
        );
        let mut obs = JsonlObserver::new(JsonlObserverOptions {
            projects_dir: projects,
            ..Default::default()
        });
        obs.open().unwrap();
        let snaps = obs.poll_once().unwrap();
        assert_eq!(snaps.len(), 1);
        assert_eq!(snaps[0].title, "ok");
    }

    #[test]
    fn exclude_workspace_filters_sessions() {
        let tmp = tempdir().unwrap();
        let projects = tmp.path().join("projects");
        let ws = projects.join("ws");
        fs::create_dir_all(&ws).unwrap();
        write_session(
            &ws,
            "s1",
            &[r#"{"timestamp":1,"type":"message","cwd":"/Users/x/WorkBuddy/secret"}"#],
        );
        let mut obs = JsonlObserver::new(JsonlObserverOptions {
            projects_dir: projects,
            exclude_workspaces: vec!["secret".into()],
            ..Default::default()
        });
        obs.open().unwrap();
        assert!(obs.poll_once().unwrap().is_empty());
    }

    #[test]
    fn db_custom_title_overrides_jsonl_ai_title() {
        // Regression: user rename only lands in workbuddy.db, not jsonl.
        let tmp = tempdir().unwrap();
        let projects = tmp.path().join("projects");
        let ws = projects.join("ws");
        fs::create_dir_all(&ws).unwrap();
        write_session(
            &ws,
            "s1",
            &[
                r#"{"timestamp":1000,"type":"ai-title","aiTitle":"打招呼","cwd":"/Users/x/WorkBuddy/t"}"#,
            ],
        );

        let db = tmp.path().join("workbuddy.db");
        {
            use rusqlite::Connection;
            let conn = Connection::open(&db).unwrap();
            conn.execute_batch(
                "CREATE TABLE sessions (
                    id TEXT PRIMARY KEY,
                    title TEXT,
                    custom_title TEXT,
                    cwd TEXT,
                    is_playground INTEGER,
                    deleted_at INTEGER
                );
                INSERT INTO sessions (id, title, custom_title, cwd, is_playground, deleted_at)
                VALUES ('s1', '打招呼', 'ai', '/Users/x/WorkBuddy/t', 1, NULL);",
            )
            .unwrap();
        }

        let mut obs = JsonlObserver::new(JsonlObserverOptions {
            projects_dir: projects,
            db_path: db,
            ..Default::default()
        });
        obs.open().unwrap();
        let snaps = obs.poll_once().unwrap();
        assert_eq!(snaps.len(), 1);
        assert_eq!(snaps[0].title, "ai");
        // is_playground=1 → 任务; label is the session title.
        assert_eq!(snaps[0].project_category, Some(ProjectCategory::Task),);
        assert_eq!(snaps[0].project_label.as_deref(), Some("ai"));
    }

    /// An automation session should be classified as 自动化 and labelled by the
    /// `automations.name` (via cwd reverse-lookup), not its folder name.
    #[test]
    fn automation_session_classified_and_named_from_table() {
        let tmp = tempdir().unwrap();
        let projects = tmp.path().join("projects");
        let ws = projects.join("automation-2026-07-17-10-27-12");
        fs::create_dir_all(&ws).unwrap();
        write_session(
            &ws,
            "s1",
            &[
                r#"{"timestamp":1,"type":"ai-title","aiTitle":"x","cwd":"/Users/x/WorkBuddy/automation-2026-07-17-10-27-12"}"#,
            ],
        );

        let db = tmp.path().join("workbuddy.db");
        {
            use rusqlite::Connection;
            let conn = Connection::open(&db).unwrap();
            conn.execute_batch(
                "CREATE TABLE sessions (
                    id TEXT PRIMARY KEY,
                    title TEXT,
                    custom_title TEXT,
                    cwd TEXT,
                    is_playground INTEGER,
                    is_background_automation INTEGER,
                    deleted_at INTEGER
                );
                INSERT INTO sessions (id, title, custom_title, cwd, is_playground, is_background_automation, deleted_at)
                VALUES ('s1','x',NULL,'/Users/x/WorkBuddy/automation-2026-07-17-10-27-12',0,1,NULL);
                CREATE TABLE automations (id TEXT PRIMARY KEY, name TEXT, cwds TEXT, deleted_at INTEGER);
                INSERT INTO automations (id, name, cwds, deleted_at)
                VALUES ('a1','每日 AI 新闻推送','[\"/Users/x/WorkBuddy/automation-2026-07-17-10-27-12\"]',NULL);",
            )
            .unwrap();
        }

        let mut obs = JsonlObserver::new(JsonlObserverOptions {
            projects_dir: projects,
            db_path: db,
            ..Default::default()
        });
        obs.open().unwrap();
        let snaps = obs.poll_once().unwrap();
        assert_eq!(snaps.len(), 1);
        assert_eq!(snaps[0].project_category, Some(ProjectCategory::Automation));
        assert_eq!(snaps[0].project_label.as_deref(), Some("每日 AI 新闻推送"));
    }

    /// A real-path project session (no playground/automation flag) classifies as
    /// 项目 and uses the folder leaf as the label.
    #[test]
    fn project_session_classified_with_folder_leaf() {
        let tmp = tempdir().unwrap();
        let projects = tmp.path().join("projects");
        let ws = projects.join("modjing");
        fs::create_dir_all(&ws).unwrap();
        write_session(
            &ws,
            "s1",
            &[
                r#"{"timestamp":1,"type":"ai-title","aiTitle":"fix bug","cwd":"/Users/x/Desktop/modjing"}"#,
            ],
        );

        let mut obs = JsonlObserver::new(JsonlObserverOptions {
            projects_dir: projects,
            ..Default::default()
        });
        obs.open().unwrap();
        let snaps = obs.poll_once().unwrap();
        assert_eq!(snaps.len(), 1);
        assert_eq!(snaps[0].project_category, Some(ProjectCategory::Project));
        assert_eq!(snaps[0].project_label.as_deref(), Some("modjing"));
    }

    /// Soft-deleted sessions must not surface even if their jsonl still exists.
    #[test]
    fn soft_deleted_session_hidden_from_poll() {
        let tmp = tempdir().unwrap();
        let projects = tmp.path().join("projects");
        let ws = projects.join("ws");
        fs::create_dir_all(&ws).unwrap();
        write_session(
            &ws,
            "s1",
            &[r#"{"timestamp":1,"type":"message","cwd":"/Users/x/WorkBuddy/t"}"#],
        );
        write_session(
            &ws,
            "s2",
            &[r#"{"timestamp":2,"type":"message","cwd":"/Users/x/WorkBuddy/t"}"#],
        );

        let db = tmp.path().join("workbuddy.db");
        {
            use rusqlite::Connection;
            let conn = Connection::open(&db).unwrap();
            conn.execute_batch(
                "CREATE TABLE sessions (
                    id TEXT PRIMARY KEY,
                    title TEXT,
                    custom_title TEXT,
                    cwd TEXT,
                    is_playground INTEGER,
                    deleted_at INTEGER
                );
                INSERT INTO sessions (id, title, custom_title, cwd, is_playground, deleted_at)
                VALUES ('s1',NULL,NULL,NULL,0,1700000000);
                INSERT INTO sessions (id, title, custom_title, cwd, is_playground, deleted_at)
                VALUES ('s2',NULL,NULL,NULL,0,NULL);",
            )
            .unwrap();
        }

        let mut obs = JsonlObserver::new(JsonlObserverOptions {
            projects_dir: projects,
            db_path: db,
            ..Default::default()
        });
        obs.open().unwrap();
        let snaps = obs.poll_once().unwrap();
        // Only s2 survives — s1 is soft-deleted.
        assert_eq!(snaps.len(), 1);
        assert_eq!(snaps[0].session_id, "s2");
    }
}
