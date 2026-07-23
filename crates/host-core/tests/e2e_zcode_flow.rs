//! E2E: fixture sqlite → observer → board → led/board assertions
//! Ported semantics from packages/host/test/e2e/zcode-flow.test.ts

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
  workspace_identity TEXT,
  task_id TEXT NOT NULL,
  title TEXT NOT NULL DEFAULT '',
  task_status TEXT,
  provider TEXT,
  mode TEXT NOT NULL DEFAULT 'build',
  model TEXT,
  migration_source TEXT,
  forked_from_task_id TEXT,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  unread_at INTEGER,
  pinned INTEGER NOT NULL DEFAULT 0,
  archived INTEGER NOT NULL DEFAULT 0,
  deleted INTEGER NOT NULL DEFAULT 0,
  title_overridden INTEGER NOT NULL DEFAULT 0,
  meta_json TEXT NOT NULL DEFAULT '{}',
  searchable_text TEXT NOT NULL DEFAULT '',
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
  turn_id text,
  trace_id text,
  tool_call_id text not null,
  tool_name text not null,
  side_effect_scope text,
  read_only integer,
  destructive integer,
  approval_status text,
  status text not null check(status in ('running', 'completed', 'error', 'cancelled')),
  started_at integer not null,
  first_output_at integer,
  completed_at integer,
  duration_ms integer,
  time_to_first_output_ms integer,
  exit_code integer,
  output_bytes integer not null default 0,
  stdout_bytes integer not null default 0,
  stderr_bytes integer not null default 0,
  truncated integer not null default 0,
  retry_count integer not null default 0,
  retryable integer not null default 0,
  cancelled_by_user integer not null default 0,
  error_type text,
  error_code text,
  error_message text
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

    fn insert_task(&self, id: &str, status: &str, workspace: &str, updated_at: u64) {
        self.tasks_db
            .execute(
                "INSERT INTO tasks (workspace_key, workspace_path, task_id, title, task_status, created_at, updated_at, deleted, archived)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0, 0)",
                rusqlite::params![
                    format!("ws:{id}"),
                    workspace,
                    id,
                    format!("task {id}"),
                    status,
                    updated_at as i64,
                    updated_at as i64
                ],
            )
            .unwrap();
    }

    fn update_task(&self, id: &str, status: &str, updated_at: u64) {
        self.tasks_db
            .execute(
                "UPDATE tasks SET task_status = ?1, updated_at = ?2 WHERE task_id = ?3",
                rusqlite::params![status, updated_at as i64, id],
            )
            .unwrap();
    }

    #[allow(clippy::too_many_arguments)]
    fn insert_tool(
        &self,
        id: &str,
        session_id: &str,
        tool_name: &str,
        scope: &str,
        approval: &str,
        status: &str,
        started_at: u64,
        completed_at: Option<u64>,
    ) {
        self.tool_db
            .execute(
                "INSERT INTO tool_usage (id, session_id, tool_call_id, tool_name, side_effect_scope, approval_status, status, started_at, completed_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                rusqlite::params![
                    id,
                    session_id,
                    format!("call_{id}"),
                    tool_name,
                    scope,
                    approval,
                    status,
                    started_at as i64,
                    completed_at.map(|v| v as i64)
                ],
            )
            .unwrap();
    }

    fn complete_tool(&self, id: &str) {
        self.tool_db
            .execute(
                "UPDATE tool_usage SET status='completed', completed_at=?1, approval_status='approved' WHERE id=?2",
                rusqlite::params![now_ms() as i64, id],
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
            enable_workbuddy: false,
            workbuddy_projects_dir: None,
        })
        .unwrap()
    }
}

fn board_status(host: &HostCore, session_id: &str) -> Option<DeckStatus> {
    host.board_state()?
        .slots
        .iter()
        .find(|s| s.session_id.as_deref() == Some(session_id))
        .map(|s| s.status)
}

#[test]
fn working_task_paints_blue_breathe() {
    let fx = Fixture::new();
    fx.insert_task("sess_working", "running", "/tmp/ws", now_ms());
    let mut host = fx.host();
    let (led, board) = host.tick().unwrap();
    let board = board.unwrap();
    assert!(board.slots.iter().any(
        |s| s.session_id.as_deref() == Some("sess_working") && s.status == DeckStatus::Working
    ));
    let led = led.unwrap();
    let occupied = led.slots.iter().find(|s| s.rgb.is_some()).unwrap();
    assert_eq!(occupied.fx, LedFx::Breathe);
    let rgb = occupied.rgb.unwrap();
    assert!(rgb[0] < 120);
    assert!(rgb[2] > 200);
}

#[test]
fn waiting_requested_is_orange_warm() {
    let fx = Fixture::new();
    let t = now_ms();
    fx.insert_task("sess_waiting", "running", "/tmp/ws", t);
    fx.insert_tool(
        "tu1",
        "sess_waiting",
        "Bash",
        "shell",
        "requested",
        "running",
        t,
        None,
    );
    let mut host = fx.host();
    host.tick().unwrap();
    assert_eq!(
        board_status(&host, "sess_waiting"),
        Some(DeckStatus::Waiting)
    );
    let led = host.led_frame().unwrap();
    let warm = led
        .slots
        .iter()
        .find(|s| s.rgb.map(|c| c[0] > 150).unwrap_or(false));
    assert!(warm.is_some(), "expected warm waiting color");
}

#[test]
fn full_demo_working_waiting_accept_done() {
    let fx = Fixture::new();
    let t0 = now_ms();
    fx.insert_task("sess_flow", "running", "/tmp/ws", t0);
    let mut host = fx.host();
    host.tick().unwrap();
    assert_eq!(board_status(&host, "sess_flow"), Some(DeckStatus::Working));

    // Started 4+ minutes ago so that once completed it falls OUTSIDE the
    // active window (RECENT_ACTIVITY_SECS = 3min). Otherwise the completed
    // tool's started_at is "recent" and active stays true → Working, never
    // Done. This models a session whose last tool call genuinely finished a
    // while ago (truly idle), not one mid-conversation.
    let tool_started = now_ms().saturating_sub(4 * 60 * 1000);
    fx.insert_tool(
        "tu_flow",
        "sess_flow",
        "Bash",
        "git push",
        "requested",
        "running",
        tool_started,
        None,
    );
    host.tick().unwrap();
    assert_eq!(board_status(&host, "sess_flow"), Some(DeckStatus::Waiting));

    fx.complete_tool("tu_flow");
    host.tick().unwrap();
    assert_eq!(board_status(&host, "sess_flow"), Some(DeckStatus::Working));

    fx.update_task("sess_flow", "completed", now_ms());
    host.tick().unwrap();
    assert_eq!(board_status(&host, "sess_flow"), Some(DeckStatus::Done));

    let led = host.led_frame().unwrap();
    let done = led.slots.iter().find(|s| s.rgb.is_some()).unwrap();
    assert_eq!(done.fx, LedFx::Solid);
    let rgb = done.rgb.unwrap();
    assert!(rgb[1] > 200);
}

#[test]
fn error_task_is_red() {
    let fx = Fixture::new();
    fx.insert_task("sess_error", "error", "/tmp/ws", now_ms());
    let mut host = fx.host();
    host.tick().unwrap();
    assert_eq!(board_status(&host, "sess_error"), Some(DeckStatus::Error));
    let led = host.led_frame().unwrap();
    let err = led.slots.iter().find(|s| s.rgb.is_some()).unwrap();
    let rgb = err.rgb.unwrap();
    assert!(rgb[0] > 200);
    assert!(rgb[1] < 80);
    assert_eq!(err.fx, LedFx::Solid);
}

#[test]
fn exclude_workspace_hides_self() {
    let fx = Fixture::new();
    fx.insert_task("sess_self", "running", "/self/excluded/project", now_ms());
    fx.insert_task("sess_other", "running", "/somewhere/else", now_ms());
    let mut host = HostCore::new(HostConfig {
        tasks_db_path: fx.tasks.clone(),
        tool_db_path: fx.tool.clone(),
        exclude_workspaces: vec!["/self/excluded".into()],
        exclude_task_ids: vec![],
        slot_count: 5,
        enable_codex: false,
        codex_cli_path: None,
        enable_workbuddy: false,
        workbuddy_projects_dir: None,
    })
    .unwrap();
    host.tick().unwrap();
    let board = host.board_state().unwrap();
    let ids: Vec<_> = board
        .slots
        .iter()
        .filter_map(|s| s.session_id.clone())
        .collect();
    assert!(!ids.contains(&"sess_self".to_string()));
    assert!(ids.contains(&"sess_other".to_string()));
}

#[test]
fn stale_requested_is_not_waiting() {
    let fx = Fixture::new();
    let recent = now_ms();
    let stale = recent - 31 * 60 * 1000;
    fx.insert_task("sess_stale", "running", "/tmp/ws", recent);
    fx.insert_tool(
        "tu_stale",
        "sess_stale",
        "Bash",
        "shell",
        "requested",
        "running",
        stale,
        None,
    );
    let mut host = fx.host();
    host.tick().unwrap();
    assert_eq!(board_status(&host, "sess_stale"), Some(DeckStatus::Working));
}

#[test]
fn leds_frame_has_five_slots() {
    let fx = Fixture::new();
    fx.insert_task("sess_shape", "running", "/tmp/ws", now_ms());
    let mut host = fx.host();
    let (led, _) = host.tick().unwrap();
    let led = led.unwrap();
    assert_eq!(led.msg_type, "leds");
    assert_eq!(led.slots.len(), 5);
    for s in &led.slots {
        if let Some(rgb) = s.rgb {
            for c in rgb {
                // u8 always in 0..=255
                let _ = c;
            }
        }
    }
}
