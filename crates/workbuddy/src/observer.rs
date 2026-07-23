//! Read-only WorkBuddy jsonl observer.
//!
//! Each WorkBuddy task is a `<session-id>.jsonl` file under
//! `~/.workbuddy/projects/<workspace>/`. This observer scans that tree
//! read-only, decodes the event stream per file, and maps it to
//! `SessionSnapshot`s. Structure mirrors the zcode `SqliteObserver`: an
//! options struct with `~/.workbuddy` defaults, an `open()` that probes the
//! tree exists, and three poll tiers (board / catalog / pinned).
//!
//! Like zcode, a missing tree degrades to an empty observer (returns
//! `Ok(vec![])`) rather than an error — one unavailable backend must never
//! break the others.

use crate::mapper::{aggregate, map_session, SessionEvent};
use agent_deck_protocol::SessionSnapshot;
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
    pub exclude_workspaces: Vec<String>,
    pub exclude_task_ids: Vec<String>,
    /// Fail (instead of degrade) when the tree is absent. Tests use this.
    pub fail_on_missing: bool,
    pub max_sessions: usize,
    pub catalog_max_sessions: usize,
}

impl Default for JsonlObserverOptions {
    fn default() -> Self {
        let home = dirs_home();
        Self {
            projects_dir: home.join(".workbuddy/projects"),
            exclude_workspaces: vec![],
            exclude_task_ids: vec![],
            fail_on_missing: false,
            max_sessions: 20,
            catalog_max_sessions: 500,
        }
    }
}

fn dirs_home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
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
                if let Some(snap) = self.read_session(&path, &session_id, now) {
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
        let signals = aggregate(&events, session_id);
        Some(map_session(&signals, now))
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
    use agent_deck_protocol::BackendId;
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
}
