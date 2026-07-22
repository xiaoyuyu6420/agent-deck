//! Codex backend graceful degradation: missing CLI must not break zcode.

use agent_deck_host_core::{DesktopService, HostConfig};
use rusqlite::Connection;
use std::path::PathBuf;
use tempfile::tempdir;

fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let dir = tempdir().unwrap();
    let tasks = dir.path().join("tasks-index.sqlite");
    let tool = dir.path().join("db.sqlite");
    let tasks_db = Connection::open(&tasks).unwrap();
    tasks_db
        .execute_batch(
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
        )
        .unwrap();
    let tool_db = Connection::open(&tool).unwrap();
    tool_db
        .execute_batch(
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
        )
        .unwrap();
    drop(tasks_db);
    drop(tool_db);
    (dir, tasks, tool)
}

#[test]
fn missing_codex_cli_does_not_break_zcode() {
    let (dir, tasks, tool) = fixture();
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let tasks_db = Connection::open(&tasks).unwrap();
    tasks_db
        .execute(
            "INSERT INTO tasks (workspace_key, workspace_path, task_id, title, task_status, created_at, updated_at, deleted, archived)
             VALUES ('ws:z','/tmp','sess_z','Zcode only','running',?1,?1,0,0)",
            rusqlite::params![t as i64],
        )
        .unwrap();
    drop(tasks_db);

    // Point at a non-existent binary — codex open fails, zcode still works.
    let mut svc = DesktopService::new_with_pins_path(
        HostConfig {
            tasks_db_path: tasks,
            tool_db_path: tool,
            exclude_workspaces: vec![],
            exclude_task_ids: vec![],
            slot_count: 5,
            enable_codex: true,
            codex_cli_path: Some(dir.path().join("no-such-codex")),
        },
        None,
    )
    .unwrap();
    svc.set_auto_fill(true);
    svc.tick_at(t).unwrap();
    assert!(!svc.using_demo());
    let board = svc.board_state();
    assert!(board
        .slots
        .iter()
        .any(|s| s.session_id.as_deref() == Some("sess_z")));
}
