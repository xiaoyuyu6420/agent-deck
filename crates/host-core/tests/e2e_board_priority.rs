//! E2E extensions: multi-session priority, urgency progression, disappearance.

use agent_deck_host_core::{HostConfig, HostCore};
use agent_deck_protocol::{DeckStatus, LedFx};
use rusqlite::Connection;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use tempfile::tempdir;

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

struct Fixture {
    #[allow(dead_code)]
    dir: tempfile::TempDir,
    tasks: PathBuf,
    tool: PathBuf,
    tasks_db: Connection,
    tool_db: Connection,
}

impl Fixture {
    fn new() -> Self {
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
        Self {
            dir,
            tasks,
            tool,
            tasks_db,
            tool_db,
        }
    }

    fn insert_task(&self, id: &str, status: &str, updated_at: u64) {
        self.tasks_db
            .execute(
                "INSERT INTO tasks (workspace_key, workspace_path, task_id, title, task_status, created_at, updated_at, deleted, archived)
                 VALUES (?1, '/tmp/ws', ?2, ?3, ?4, ?5, ?5, 0, 0)",
                rusqlite::params![
                    format!("ws:{id}"),
                    id,
                    format!("task {id}"),
                    status,
                    updated_at as i64
                ],
            )
            .unwrap();
    }

    fn insert_waiting_tool(&self, id: &str, session_id: &str, tool: &str, scope: &str, at: u64) {
        self.tool_db
            .execute(
                "INSERT INTO tool_usage (id, session_id, tool_call_id, tool_name, side_effect_scope, approval_status, status, started_at, completed_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, 'requested', 'running', ?6, NULL)",
                rusqlite::params![id, session_id, format!("call_{id}"), tool, scope, at as i64],
            )
            .unwrap();
    }

    fn delete_task(&self, id: &str) {
        self.tasks_db
            .execute(
                "DELETE FROM tasks WHERE task_id = ?1",
                rusqlite::params![id],
            )
            .unwrap();
    }

    fn host(&self) -> HostCore {
        HostCore::new(HostConfig {
            tasks_db_path: self.tasks.clone(),
            tool_db_path: self.tool.clone(),
            exclude_workspaces: vec![],
            exclude_task_ids: vec![],
            slot_count: 5,
            enable_codex: false,
            codex_cli_path: None,
        })
        .unwrap()
    }
}

#[test]
fn waiting_outranks_working_and_done() {
    let fx = Fixture::new();
    // tool_usage stale window uses SQL real time — timestamps must be recent
    let t = now_ms();
    fx.insert_task("done1", "completed", t);
    fx.insert_task("work1", "running", t + 1);
    fx.insert_task("wait1", "running", t + 2);
    fx.insert_waiting_tool("tu1", "wait1", "AskUserQuestion", "userInteraction", t + 2);

    let mut host = fx.host();
    host.tick_at(t + 2).unwrap();
    let board = host.board_state().unwrap();
    assert_eq!(board.slots[0].session_id.as_deref(), Some("wait1"));
    assert_eq!(board.slots[0].status, DeckStatus::Waiting);
    assert_eq!(board.slots[1].session_id.as_deref(), Some("work1"));
    assert_eq!(board.slots[1].status, DeckStatus::Working);
    assert_eq!(board.slots[2].session_id.as_deref(), Some("done1"));
    assert_eq!(board.slots[2].status, DeckStatus::Done);
}

#[test]
fn urgency_low_risk_starts_solid_then_blink_fast() {
    let fx = Fixture::new();
    // waiting_since comes from task.updated_at; keep it equal to paint clock origin
    let t0 = now_ms();
    fx.insert_task("urg", "running", t0);
    fx.insert_waiting_tool("tu", "urg", "AskUserQuestion", "userInteraction", t0);

    let mut host = fx.host();
    host.tick_at(t0).unwrap();
    assert_eq!(
        host.board_state()
            .unwrap()
            .slots
            .iter()
            .find(|s| s.session_id.as_deref() == Some("urg"))
            .map(|s| s.status),
        Some(DeckStatus::Waiting)
    );
    let led0 = host.led_frame().unwrap();
    let slot0 = led0.slots.iter().find(|s| s.rgb.is_some()).unwrap();
    assert_eq!(slot0.fx, LedFx::Solid, "fresh low-risk waiting is solid");

    // advance 3 minutes without re-polling SQL; recompute only
    host.recompute_at(t0 + 3 * 60 * 1000);
    let led1 = host.led_frame().unwrap();
    let slot1 = led1.slots.iter().find(|s| s.rgb.is_some()).unwrap();
    assert_eq!(slot1.fx, LedFx::BlinkFast);
    let rgb = slot1.rgb.unwrap();
    assert!(rgb[0] > 240);
    assert!(rgb[1] < 80);
}

#[test]
fn more_than_five_sessions_only_top_five_slots() {
    let fx = Fixture::new();
    let t = now_ms();
    for i in 0..7 {
        fx.insert_task(&format!("s{i}"), "running", t + i as u64);
    }
    let mut host = fx.host();
    host.tick_at(t + 10).unwrap();
    let board = host.board_state().unwrap();
    let occupied = board
        .slots
        .iter()
        .filter(|s| s.session_id.is_some())
        .count();
    assert_eq!(occupied, 5);
    assert_eq!(board.slots.len(), 5);
}

#[test]
fn removed_task_disappears_from_board() {
    let fx = Fixture::new();
    let t = now_ms();
    fx.insert_task("gone", "running", t);
    let mut host = fx.host();
    host.tick_at(t).unwrap();
    assert!(host
        .board_state()
        .unwrap()
        .slots
        .iter()
        .any(|s| s.session_id.as_deref() == Some("gone")));

    fx.delete_task("gone");
    host.tick_at(t + 1).unwrap();
    assert!(host
        .board_state()
        .unwrap()
        .slots
        .iter()
        .all(|s| s.session_id.as_deref() != Some("gone")));
}

#[test]
fn focus_marks_slot_and_persists_across_tick() {
    let fx = Fixture::new();
    let t = now_ms();
    fx.insert_task("a", "running", t);
    fx.insert_task("b", "running", t + 1);
    let mut host = fx.host();
    host.tick_at(t + 1).unwrap();
    host.set_focus_at(1, t + 1);
    assert_eq!(host.board_state().unwrap().focus, 1);
    host.tick_at(t + 2).unwrap();
    assert_eq!(host.board_state().unwrap().focus, 1);
    assert_eq!(host.board_state().unwrap().slots[1].focused, Some(true));
}
