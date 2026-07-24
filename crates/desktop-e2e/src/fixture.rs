//! Isolated data fixture for UI e2e.
//!
//! Spins up temp SQLite DBs shaped like the real zcode schema, returns their
//! paths so the harness can inject them into the app via AGENT_DECK_TASKS_DB /
//! AGENT_DECK_TOOL_DB env vars. The TempDir guard must outlive the app process.

use anyhow::Result;
use rusqlite::Connection;
use std::path::PathBuf;
use tempfile::{tempdir, TempDir};

/// A self-contained zcode-shaped fixture. Keep `_dir` alive for the test's
/// lifetime or the temp files vanish mid-run.
pub struct Fixture {
    _dir: TempDir,
    pub tasks_db: PathBuf,
    pub tool_db: PathBuf,
}

impl Fixture {
    pub fn empty() -> Result<Self> {
        let dir = tempdir()?;
        let tasks_db = dir.path().join("tasks-index.sqlite");
        let tool_db = dir.path().join("db.sqlite");

        let conn = Connection::open(&tasks_db)?;
        conn.execute_batch(
            r#"
CREATE TABLE tasks (
  workspace_key TEXT NOT NULL,
  workspace_path TEXT NOT NULL,
  task_id TEXT NOT NULL,
  title TEXT NOT NULL DEFAULT '',
  task_status TEXT,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  archived INTEGER NOT NULL DEFAULT 0,
  deleted INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (workspace_key, task_id)
);
"#,
        )?;
        drop(conn);

        let conn = Connection::open(&tool_db)?;
        conn.execute_batch(
            r#"
CREATE TABLE tool_usage (
  id text primary key,
  session_id text not null,
  tool_call_id text not null,
  tool_name text not null,
  side_effect_scope text,
  approval_status text,
  status text not null,
  started_at integer not null,
  completed_at integer
);
"#,
        )?;
        drop(conn);

        Ok(Self {
            _dir: dir,
            tasks_db,
            tool_db,
        })
    }
}
