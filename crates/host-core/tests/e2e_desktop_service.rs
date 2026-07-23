//! E2E: DesktopService command-layer (what Tauri IPC exposes)
//! Covers demo fallback, focus, action stub, real sqlite path.

use agent_deck_host_core::{DesktopService, HostConfig};
use agent_deck_protocol::{BackendId, DeckStatus, LedFx};
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
    // Disable pin persistence in tests so ~/.agent-deck/pins.json never leaks in.
    DesktopService::new_with_pins_path(
        HostConfig {
            tasks_db_path: tasks,
            tool_db_path: tool,
            exclude_workspaces: vec![],
            exclude_task_ids: vec![],
            slot_count: 5,
            enable_codex: false,
            codex_cli_path: None,
            enable_workbuddy: false,
            workbuddy_projects_dir: None,
        },
        None,
    )
    .unwrap()
}

#[test]
fn empty_db_shows_empty_board_not_demo() {
    let (_dir, tasks, tool) = empty_fixture();
    let svc = service_with(tasks, tool);
    assert!(!svc.using_demo());
    let board = svc.board_state();
    assert_eq!(board.msg_type, "board");
    // Manual-bind default: no fake demo sessions.
    assert!(board.slots.iter().all(|s| s.session_id.is_none()));
    let leds = svc.led_frame();
    assert_eq!(leds.msg_type, "leds");
    assert_eq!(leds.slots.len(), board.slots.len());
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
fn dispatch_action_unknown_returns_unsupported() {
    let (_dir, tasks, tool) = empty_fixture();
    let mut svc = service_with(tasks, tool);
    // Unknown action string is rejected at parse time.
    assert_eq!(svc.dispatch_action("bogus"), "unsupported:unknown:bogus");
}

#[test]
fn dispatch_action_without_target_slot_is_unsupported() {
    let (_dir, tasks, tool) = empty_fixture();
    let mut svc = service_with(tasks, tool);
    // Empty board, focused slot 0 has no session → no target.
    let r = svc.dispatch_action("accept");
    assert!(r.starts_with("unsupported:"), "got: {r}");
    assert!(r.contains("empty_slot") || r.contains("no_observer"), "got: {r}");
}

#[test]
fn dispatch_action_routes_to_focused_slot_backend() {
    // With only a zcode observer registered (codex/workbuddy disabled), a
    // zcode-bound slot exists but zcode's dispatch is the trait default →
    // "unsupported:accept". This proves the router reached the right backend
    // rather than the old global stub.
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
    svc.set_auto_fill(true);
    svc.tick_at(t).unwrap();
    // Slot 0 now holds the zcode session; focus defaults to 0.
    let board = svc.board_state();
    let bound = board.slots.iter().find(|s| s.session_id.is_some()).unwrap();
    assert_eq!(bound.backend, Some(BackendId::Zcode));
    let focused = board.focus;
    let r = svc.dispatch_action("stop");
    // zcode dispatch is the trait default (no write path yet) → unsupported,
    // but the tag proves routing worked (not the old "unsupported:stop" stub
    // which would also match — so we additionally assert it's NOT the empty-
    // slot path).
    assert!(r.starts_with("unsupported:stop"), "got: {r}");
    assert!(!r.contains("empty_slot"), "should have routed, got: {r}");
    let _ = (dir, focused);
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
    // Auto-fill so unbound sessions appear without manual pin.
    svc.set_auto_fill(true);
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

#[test]
fn pin_slot_marks_board_and_persists() {
    let dir = tempdir().unwrap();
    let (_fixture, tasks, tool) = empty_fixture();
    let pins_path = dir.path().join("pins.json");
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;

    let tasks_db = Connection::open(&tasks).unwrap();
    tasks_db
        .execute(
            "INSERT INTO tasks (workspace_key, workspace_path, task_id, title, task_status, created_at, updated_at, deleted, archived)
             VALUES ('ws:p','/tmp','sess_pin','Pinned','running',?1,?1,0,0)",
            rusqlite::params![t as i64],
        )
        .unwrap();
    drop(tasks_db);

    let mut svc = DesktopService::new_with_pins_path(
        HostConfig {
            tasks_db_path: tasks.clone(),
            tool_db_path: tool.clone(),
            exclude_workspaces: vec![],
            exclude_task_ids: vec![],
            slot_count: 5,
            enable_codex: false,
            codex_cli_path: None,
            enable_workbuddy: false,
            workbuddy_projects_dir: None,
        },
        Some(pins_path.clone()),
    )
    .unwrap();
    svc.tick_at(t).unwrap();
    svc.pin_slot(0, Some("sess_pin".into()));

    let board = svc.board_state();
    assert_eq!(board.slots[0].session_id.as_deref(), Some("sess_pin"));
    assert_eq!(board.slots[0].pinned, Some(true));
    assert!(pins_path.exists(), "pin_slot should write pins.json");

    // Reload from disk — pin must survive restart.
    let mut svc2 = DesktopService::new_with_pins_path(
        HostConfig {
            tasks_db_path: tasks,
            tool_db_path: tool,
            exclude_workspaces: vec![],
            exclude_task_ids: vec![],
            slot_count: 5,
            enable_codex: false,
            codex_cli_path: None,
            enable_workbuddy: false,
            workbuddy_projects_dir: None,
        },
        Some(pins_path),
    )
    .unwrap();
    svc2.tick_at(t).unwrap();
    let board2 = svc2.board_state();
    assert_eq!(board2.slots[0].pinned, Some(true));
    assert_eq!(board2.slots[0].session_id.as_deref(), Some("sess_pin"));
}
