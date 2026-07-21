//! Host orchestration: poll zcode → board → led/board state.
//! Ported from packages/host/src/main.ts (core loop only)

use agent_deck_board::SessionBoard;
use agent_deck_protocol::{BackendId, BoardState, LedFrame};
use agent_deck_zcode::{SqliteObserver, SqliteObserverOptions};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone)]
pub struct HostConfig {
    pub tasks_db_path: PathBuf,
    pub tool_db_path: PathBuf,
    pub exclude_workspaces: Vec<String>,
    pub exclude_task_ids: Vec<String>,
    pub slot_count: usize,
}

impl Default for HostConfig {
    fn default() -> Self {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        Self {
            tasks_db_path: home.join(".zcode/v2/tasks-index.sqlite"),
            tool_db_path: home.join(".zcode/cli/db/db.sqlite"),
            exclude_workspaces: vec![],
            exclude_task_ids: vec![],
            slot_count: 5,
        }
    }
}

pub struct HostCore {
    pub board: SessionBoard,
    observer: SqliteObserver,
}

impl HostCore {
    pub fn new(config: HostConfig) -> anyhow::Result<Self> {
        let mut observer = SqliteObserver::new(SqliteObserverOptions {
            tasks_db_path: config.tasks_db_path,
            tool_db_path: config.tool_db_path,
            exclude_workspaces: config.exclude_workspaces,
            exclude_task_ids: config.exclude_task_ids,
            fail_on_missing: false,
        });
        observer.open()?;
        let board = SessionBoard::new(config.slot_count);
        Ok(Self { board, observer })
    }

    /// Poll sqlite once and recompute board using wall clock.
    pub fn tick(&mut self) -> anyhow::Result<(Option<LedFrame>, Option<BoardState>)> {
        self.tick_at(now_ms())
    }

    /// Poll sqlite once and recompute board using an injected clock (for tests).
    pub fn tick_at(&mut self, now: u64) -> anyhow::Result<(Option<LedFrame>, Option<BoardState>)> {
        let snaps = self.observer.poll_once()?;
        self.board
            .replace_backend_sessions(BackendId::Zcode, snaps, now);
        Ok((
            self.board.led_frame().cloned(),
            self.board.board_state().cloned(),
        ))
    }

    pub fn recompute_only(&mut self) {
        self.recompute_at(now_ms());
    }

    pub fn recompute_at(&mut self, now: u64) {
        self.board.recompute(now);
    }

    pub fn set_focus(&mut self, i: usize) {
        self.set_focus_at(i, now_ms());
    }

    pub fn set_focus_at(&mut self, i: usize, now: u64) {
        self.board.set_focus(i);
        self.board.recompute(now);
    }

    pub fn board_state(&self) -> Option<&BoardState> {
        self.board.board_state()
    }

    pub fn led_frame(&self) -> Option<&LedFrame> {
        self.board.led_frame()
    }

    pub fn is_empty_board(&self) -> bool {
        self.board_state()
            .map(|b| b.slots.iter().all(|s| s.session_id.is_none()))
            .unwrap_or(true)
    }
}

/// Desktop-facing service: host core + demo fallback + action stubs.
/// Used by Tauri commands so behavior is unit/integration testable without GUI.
pub struct DesktopService {
    host: HostCore,
    using_demo: bool,
}

impl DesktopService {
    pub fn new(config: HostConfig) -> anyhow::Result<Self> {
        let mut host = HostCore::new(config)?;
        let _ = host.tick();
        let using_demo = host.is_empty_board();
        Ok(Self { host, using_demo })
    }

    pub fn from_host(host: HostCore) -> Self {
        let using_demo = host.is_empty_board();
        Self { host, using_demo }
    }

    pub fn tick(&mut self) -> anyhow::Result<()> {
        self.tick_at(now_ms())
    }

    pub fn tick_at(&mut self, now: u64) -> anyhow::Result<()> {
        self.host.tick_at(now)?;
        self.using_demo = self.host.is_empty_board();
        Ok(())
    }

    pub fn board_state(&self) -> BoardState {
        if self.using_demo {
            demo_board_state().1
        } else {
            self.host
                .board_state()
                .cloned()
                .unwrap_or_else(|| demo_board_state().1)
        }
    }

    pub fn led_frame(&self) -> LedFrame {
        if self.using_demo {
            demo_board_state().0
        } else {
            self.host
                .led_frame()
                .cloned()
                .unwrap_or_else(|| demo_board_state().0)
        }
    }

    pub fn set_focus(&mut self, i: usize) {
        self.host.set_focus(i);
        self.using_demo = false;
    }

    pub fn set_focus_at(&mut self, i: usize, now: u64) {
        self.host.set_focus_at(i, now);
        self.using_demo = false;
    }

    pub fn using_demo(&self) -> bool {
        self.using_demo
    }

    pub fn dispatch_action(&self, action: &str) -> String {
        // V1: actions acknowledged but unsupported until ACP attach is verified.
        format!("unsupported:{action}")
    }

    pub fn host(&self) -> &HostCore {
        &self.host
    }
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Demo board used when no real zcode data is available (Phase R0).
pub fn demo_board_state() -> (LedFrame, BoardState) {
    use agent_deck_protocol::*;
    let mut board = SessionBoard::new(5);
    let now = now_ms();
    board.replace_backend_sessions(
        BackendId::Zcode,
        vec![
            SessionSnapshot {
                backend: BackendId::Zcode,
                session_id: "demo_waiting".into(),
                title: "Demo: waiting for approval".into(),
                status: DeckStatus::Waiting,
                risk: Some(Risk::High),
                detail: Some("Bash: shell".into()),
                waiting_since: Some(now.saturating_sub(30_000)),
                updated_at: now,
                workspace_path: Some("/demo".into()),
            },
            SessionSnapshot {
                backend: BackendId::Zcode,
                session_id: "demo_working".into(),
                title: "Demo: running task".into(),
                status: DeckStatus::Working,
                risk: None,
                detail: None,
                waiting_since: None,
                updated_at: now,
                workspace_path: Some("/demo".into()),
            },
        ],
        now,
    );
    (
        board.led_frame().cloned().unwrap(),
        board.board_state().cloned().unwrap(),
    )
}
