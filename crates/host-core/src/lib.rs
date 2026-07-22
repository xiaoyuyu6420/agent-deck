//! Host orchestration: poll zcode → board → led/board state.
//! Ported from packages/host/src/main.ts (core loop only)

use agent_deck_board::SessionBoard;
use agent_deck_protocol::{BackendId, BoardState, LedFrame, SessionSnapshot};
use agent_deck_zcode::{SqliteObserver, SqliteObserverOptions};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// One backend's session observer (zcode / codex / future).
pub trait BackendObserver: Send {
    fn id(&self) -> BackendId;
    fn poll(&mut self) -> anyhow::Result<Vec<SessionSnapshot>>;
}

impl BackendObserver for SqliteObserver {
    fn id(&self) -> BackendId {
        BackendId::Zcode
    }

    fn poll(&mut self) -> anyhow::Result<Vec<SessionSnapshot>> {
        Ok(self.poll_once()?)
    }
}

impl BackendObserver for agent_deck_codex::CodexObserver {
    fn id(&self) -> BackendId {
        BackendId::Codex
    }

    fn poll(&mut self) -> anyhow::Result<Vec<SessionSnapshot>> {
        Ok(self.poll_once()?)
    }
}

#[derive(Debug, Clone)]
pub struct HostConfig {
    pub tasks_db_path: PathBuf,
    pub tool_db_path: PathBuf,
    pub exclude_workspaces: Vec<String>,
    pub exclude_task_ids: Vec<String>,
    pub slot_count: usize,
    /// When true, also register a CodexObserver (graceful no-op if CLI missing).
    pub enable_codex: bool,
    /// Override path to the codex binary; None → auto-detect.
    pub codex_cli_path: Option<PathBuf>,
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
            slot_count: 8,
            enable_codex: true,
            codex_cli_path: None,
        }
    }
}

pub struct HostCore {
    pub board: SessionBoard,
    observers: Vec<Box<dyn BackendObserver>>,
}

impl HostCore {
    pub fn new(config: HostConfig) -> anyhow::Result<Self> {
        let mut observers: Vec<Box<dyn BackendObserver>> = Vec::new();

        let mut zcode = SqliteObserver::new(SqliteObserverOptions {
            tasks_db_path: config.tasks_db_path,
            tool_db_path: config.tool_db_path,
            exclude_workspaces: config.exclude_workspaces,
            exclude_task_ids: config.exclude_task_ids,
            fail_on_missing: false,
        });
        // Missing zcode DBs is fine — observer stays empty.
        let _ = zcode.open();
        observers.push(Box::new(zcode));

        if config.enable_codex {
            let mut codex =
                agent_deck_codex::CodexObserver::new(agent_deck_codex::CodexObserverOptions {
                    cli_path: config.codex_cli_path,
                    ..Default::default()
                });
            // open is best-effort; poll will also try reconnect.
            let _ = codex.open();
            observers.push(Box::new(codex));
        }

        let board = SessionBoard::new(config.slot_count);
        Ok(Self { board, observers })
    }

    /// Construct with an explicit observer list (tests inject fixtures).
    pub fn with_observers(slot_count: usize, observers: Vec<Box<dyn BackendObserver>>) -> Self {
        Self {
            board: SessionBoard::new(slot_count),
            observers,
        }
    }

    /// Poll all backends once and recompute board using wall clock.
    pub fn tick(&mut self) -> anyhow::Result<(Option<LedFrame>, Option<BoardState>)> {
        self.tick_at(now_ms())
    }

    /// Poll all backends once and recompute board using an injected clock (for tests).
    pub fn tick_at(&mut self, now: u64) -> anyhow::Result<(Option<LedFrame>, Option<BoardState>)> {
        for obs in &mut self.observers {
            // One backend failing must not block the others.
            match obs.poll() {
                Ok(snaps) => {
                    self.board.replace_backend_sessions(obs.id(), snaps, now);
                }
                Err(_) => continue,
            }
        }
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

    /// Pin a session to slot `i` (session_id=None unpins). Delegates to the board.
    pub fn set_pin(&mut self, i: usize, session_id: Option<String>) {
        self.board.set_pin(i, session_id, now_ms());
    }

    /// Snapshot of current pins, for persistence.
    pub fn pins(&self) -> &std::collections::HashMap<usize, String> {
        self.board.pins()
    }

    /// Bulk-restore pins (e.g. on startup from disk).
    pub fn set_pins(&mut self, pins: std::collections::HashMap<usize, String>) {
        self.board.set_pins(pins, now_ms());
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
    /// Where pin map is persisted across restarts. None disables persistence.
    pins_path: Option<PathBuf>,
}

impl DesktopService {
    pub fn new(config: HostConfig) -> anyhow::Result<Self> {
        Self::new_with_pins_path(config, Some(default_pins_path()))
    }

    /// Construct with an explicit pins path (tests can inject a temp file).
    pub fn new_with_pins_path(
        config: HostConfig,
        pins_path: Option<PathBuf>,
    ) -> anyhow::Result<Self> {
        let mut host = HostCore::new(config)?;
        let _ = host.tick();
        if let Some(ref path) = pins_path {
            let pins = load_pins(path);
            if !pins.is_empty() {
                host.set_pins(pins);
            }
        }
        let using_demo = host.is_empty_board();
        Ok(Self {
            host,
            using_demo,
            pins_path,
        })
    }

    pub fn from_host(host: HostCore) -> Self {
        let using_demo = host.is_empty_board();
        Self {
            host,
            using_demo,
            pins_path: None,
        }
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

    /// Pin/unpin a session on a slot. Pinning forces real data mode (not demo).
    pub fn pin_slot(&mut self, i: usize, session_id: Option<String>) {
        self.host.set_pin(i, session_id);
        self.using_demo = false;
        if let Some(ref path) = self.pins_path {
            let _ = save_pins(path, self.host.pins());
        }
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

/// Default pin persistence path: `~/.agent-deck/pins.json`.
pub fn default_pins_path() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".agent-deck/pins.json")
}

/// Load pin map from disk. Missing/invalid file → empty map (no crash).
pub fn load_pins(path: &Path) -> HashMap<usize, String> {
    let Ok(raw) = fs::read_to_string(path) else {
        return HashMap::new();
    };
    let Ok(map) = serde_json::from_str::<HashMap<String, String>>(&raw) else {
        return HashMap::new();
    };
    map.into_iter()
        .filter_map(|(k, v)| k.parse::<usize>().ok().map(|i| (i, v)))
        .collect()
}

/// Persist pin map. Creates parent dirs if needed.
pub fn save_pins(path: &Path, pins: &HashMap<usize, String>) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let as_str: HashMap<String, String> = pins
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect();
    let raw = serde_json::to_string_pretty(&as_str)?;
    fs::write(path, raw)?;
    Ok(())
}

/// Demo board used when no real zcode data is available (Phase R0).
pub fn demo_board_state() -> (LedFrame, BoardState) {
    use agent_deck_protocol::*;
    let mut board = SessionBoard::new(SLOT_COUNT);
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
