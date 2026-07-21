//! E2E: DesktopService command-layer (what Tauri IPC exposes)
//! Covers demo fallback, focus, action stub, real sqlite path.

use agent_deck_host_core::{DesktopService, HostConfig};
use agent_deck_protocol::{DeckStatus, LedFx};
use rusqlite::Connection;
use std::path::PathBuf;
use tempfile::tempdir;

fn empty_fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
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

fn service_with(tasks: PathBuf, tool: PathBuf) -> DesktopService {
    DesktopService::new(HostConfig {
        tasks_db_path: tasks,
        tool_db_path: tool,
        exclude_workspaces: vec![],
        exclude_task_ids: vec![],
        slot_count: 5,
    })
    .unwrap()
}

#[test]
fn empty_db_falls_back_to_demo() {
    let (_dir, tasks, tool) = empty_fixture();
    let svc = service_with(tasks, tool);
    assert!(svc.using_demo());
    let board = svc.board_state();
    assert_eq!(board.msg_type, "board");
    assert_eq!(board.slots.len(), 5);
    let occupied: Vec<_> = board
        .slots
        .iter()
        .filter(|s| s.session_id.is_some())
        .collect();
    assert!(occupied.len() >= 2, "demo should show sample sessions");
    let leds = svc.led_frame();
    assert_eq!(leds.msg_type, "leds");
    assert_eq!(leds.slots.len(), 5);
}

#[test]
fn set_focus_updates_board_focus() {
    let (dir, tasks, tool) = empty_fixture();
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let tasks_db = Connection::open(&tasks).unwrap();
    tasks_db
        .execute(
            "INSERT INTO tasks (workspace_key, workspace_path, task_id, title, task_status, created_at, updated_at, deleted, archived)
             VALUES ('ws:a','/tmp','sess_a','A','running',?1,?1,0,0)",
            rusqlite::params![t as i64],
        )
        .unwrap();
    drop(tasks_db);

    let mut svc = service_with(tasks, tool);
    svc.tick_at(t).unwrap();
    svc.set_focus_at(1, t);
    let board = svc.board_state();
    assert_eq!(board.focus, 1);
    assert_eq!(board.slots[1].focused, Some(true));
    let _ = dir;
}

#[test]
fn dispatch_action_is_unsupported_v1() {
    let (_dir, tasks, tool) = empty_fixture();
    let svc = service_with(tasks, tool);
    assert_eq!(svc.dispatch_action("accept"), "unsupported:accept");
    assert_eq!(svc.dispatch_action("reject"), "unsupported:reject");
    assert_eq!(svc.dispatch_action("stop"), "unsupported:stop");
}

#[test]
fn real_sqlite_path_disables_demo() {
    let (dir, tasks, tool) = empty_fixture();
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let tasks_db = Connection::open(&tasks).unwrap();
    tasks_db
        .execute(
            "INSERT INTO tasks (workspace_key, workspace_path, task_id, title, task_status, created_at, updated_at, deleted, archived)
             VALUES ('ws:r','/tmp','sess_real','Real','running',?1,?1,0,0)",
            rusqlite::params![t as i64],
        )
        .unwrap();
    drop(tasks_db);

    let mut svc = service_with(tasks, tool);
    svc.tick_at(t).unwrap();
    assert!(!svc.using_demo());
    let board = svc.board_state();
    assert!(board
        .slots
        .iter()
        .any(|s| s.session_id.as_deref() == Some("sess_real") && s.status == DeckStatus::Working));
    let leds = svc.led_frame();
    let occupied = leds.slots.iter().find(|s| s.rgb.is_some()).unwrap();
    assert_eq!(occupied.fx, LedFx::Breathe);
    let _ = dir;
}
