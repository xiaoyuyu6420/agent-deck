//! Host orchestration: poll zcode → board → led/board state.
//! Ported from packages/host/src/main.ts (core loop only)

use agent_deck_board::SessionBoard;
use agent_deck_protocol::{BackendId, BoardState, DeckStatus, LedFrame, SessionSnapshot};
use agent_deck_zcode::{SqliteObserver, SqliteObserverOptions};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Lightweight session row for the bind-picker UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub backend: BackendId,
    pub session_id: String,
    pub title: String,
    pub status: DeckStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub updated_at: u64,
}

impl From<&SessionSnapshot> for SessionInfo {
    fn from(s: &SessionSnapshot) -> Self {
        Self {
            backend: s.backend,
            session_id: s.session_id.clone(),
            title: s.title.clone(),
            status: s.status,
            workspace_path: s.workspace_path.clone(),
            detail: s.detail.clone(),
            updated_at: s.updated_at,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct DeckSettings {
    /// When true, unbound slots auto-fill by priority. When false (default),
    /// only manually pinned sessions show — empty keys stay Off.
    #[serde(default)]
    pub auto_fill: bool,
}

/// One backend's session observer (zcode / codex / future).
pub trait BackendObserver: Send {
    fn id(&self) -> BackendId;
    fn poll(&mut self) -> anyhow::Result<Vec<SessionSnapshot>>;
    /// Full history catalog for bind picker. Defaults to poll() result.
    fn list_catalog(&mut self) -> anyhow::Result<Vec<SessionSnapshot>> {
        self.poll()
    }
    /// Fetch the *latest* state of the given session ids, bypassing the active
    /// poll window / status filter / LIMIT. Used to keep manually pinned
    /// sessions live even when they fall outside `poll()`'s recent-20 window.
    ///
    /// Each observer only returns rows it actually has (so callers can safely
    /// pass ids belonging to other backends — they just won't match). Default
    /// falls back to filtering `poll()` by id, which preserves old behavior.
    fn poll_pinned(&mut self, ids: &[String]) -> anyhow::Result<Vec<SessionSnapshot>> {
        let snaps = self.poll()?;
        Ok(snaps
            .into_iter()
            .filter(|s| ids.iter().any(|id| id == &s.session_id))
            .collect())
    }
}

impl BackendObserver for SqliteObserver {
    fn id(&self) -> BackendId {
        BackendId::Zcode
    }

    fn poll(&mut self) -> anyhow::Result<Vec<SessionSnapshot>> {
        Ok(self.poll_once()?)
    }

    fn list_catalog(&mut self) -> anyhow::Result<Vec<SessionSnapshot>> {
        Ok(self.catalog_once()?)
    }

    fn poll_pinned(&mut self, ids: &[String]) -> anyhow::Result<Vec<SessionSnapshot>> {
        Ok(self.poll_pinned_once(ids)?)
    }
}

impl BackendObserver for agent_deck_codex::CodexObserver {
    fn id(&self) -> BackendId {
        BackendId::Codex
    }

    fn poll(&mut self) -> anyhow::Result<Vec<SessionSnapshot>> {
        Ok(self.poll_once()?)
    }

    fn list_catalog(&mut self) -> anyhow::Result<Vec<SessionSnapshot>> {
        Ok(self.catalog_once()?)
    }

    fn poll_pinned(&mut self, ids: &[String]) -> anyhow::Result<Vec<SessionSnapshot>> {
        Ok(self.poll_pinned_once(ids)?)
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
        // Snapshot the current pinned ids up-front (before any recompute) so we
        // can refresh them outside the active poll window.
        let pinned_ids: Vec<String> = self.board.pins().values().cloned().collect();

        for obs in &mut self.observers {
            // One backend failing must not block the others.
            match obs.poll() {
                Ok(snaps) => {
                    self.board.replace_backend_sessions(obs.id(), snaps, now);
                }
                Err(_) => continue,
            }
            // Refresh pinned sessions' latest state even if they fell out of
            // poll()'s recent-20 window. poll_pinned only returns rows this
            // backend actually owns, so it's safe to call with all ids.
            // Batched upsert recomputes once, overriding any stale cached row
            // for a bound id with fresh state — this is what keeps a bound key
            // live instead of frozen at its bind-time snapshot.
            if !pinned_ids.is_empty() {
                if let Ok(pinned_snaps) = obs.poll_pinned(&pinned_ids) {
                    self.board.upsert_sessions(pinned_snaps, now);
                }
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

    pub fn set_auto_fill(&mut self, enabled: bool) {
        self.board.set_auto_fill(enabled, now_ms());
    }

    pub fn auto_fill(&self) -> bool {
        self.board.auto_fill()
    }

    /// Board-cache sessions (poll window). Prefer `list_catalog` for bind UI.
    pub fn list_sessions(&self) -> Vec<SessionInfo> {
        self.board
            .list_sessions()
            .iter()
            .map(SessionInfo::from)
            .collect()
    }

    /// Full history catalog across backends for the bind picker.
    pub fn list_catalog(&mut self) -> Vec<SessionInfo> {
        let mut out: Vec<SessionInfo> = Vec::new();
        let mut seen = std::collections::HashSet::<String>::new();
        for obs in &mut self.observers {
            match obs.list_catalog() {
                Ok(snaps) => {
                    for s in snaps {
                        let key = format!("{:?}:{}", s.backend, s.session_id);
                        if seen.insert(key) {
                            out.push(SessionInfo::from(&s));
                        }
                    }
                }
                Err(_) => continue,
            }
        }
        out.sort_by(|a, b| {
            b.status
                .priority()
                .cmp(&a.status.priority())
                .then_with(|| b.updated_at.cmp(&a.updated_at))
        });
        out
    }

    /// Resolve a session from catalog (or board cache) and pin it, so historical
    /// sessions outside the poll window still bind correctly.
    pub fn pin_session_from_catalog(&mut self, i: usize, session_id: Option<String>) {
        match session_id {
            None => self.set_pin(i, None),
            Some(id) => {
                if self.board.find_session(&id).is_none() {
                    // Pull catalog and upsert the chosen historical session.
                    let mut found: Option<SessionSnapshot> = None;
                    for obs in &mut self.observers {
                        if let Ok(snaps) = obs.list_catalog() {
                            if let Some(snap) = snaps.into_iter().find(|s| s.session_id == id) {
                                found = Some(snap);
                                break;
                            }
                        }
                    }
                    if let Some(snap) = found {
                        self.board.upsert_session(snap, now_ms());
                    }
                }
                self.set_pin(i, Some(id));
            }
        }
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

/// Desktop-facing service: host core + action stubs.
/// Used by Tauri commands so behavior is unit/integration testable without GUI.
///
/// Demo fallback is OFF by default: empty keys stay Off until the user binds
/// a session (manual pin mode). Set `DeckSettings.auto_fill = true` to restore
/// priority auto-fill of unbound slots.
pub struct DesktopService {
    host: HostCore,
    /// Where pin map is persisted across restarts. None disables persistence.
    pins_path: Option<PathBuf>,
    settings_path: Option<PathBuf>,
    settings: DeckSettings,
}

impl DesktopService {
    pub fn new(config: HostConfig) -> anyhow::Result<Self> {
        Self::new_with_paths(
            config,
            Some(default_pins_path()),
            Some(default_settings_path()),
        )
    }

    /// Construct with explicit paths (tests inject temp files / disable persistence).
    pub fn new_with_pins_path(
        config: HostConfig,
        pins_path: Option<PathBuf>,
    ) -> anyhow::Result<Self> {
        Self::new_with_paths(config, pins_path, None)
    }

    pub fn new_with_paths(
        config: HostConfig,
        pins_path: Option<PathBuf>,
        settings_path: Option<PathBuf>,
    ) -> anyhow::Result<Self> {
        let mut host = HostCore::new(config)?;
        let _ = host.tick();

        let settings = settings_path
            .as_ref()
            .map(|p| load_settings(p))
            .unwrap_or_default();
        host.set_auto_fill(settings.auto_fill);

        if let Some(ref path) = pins_path {
            let pins = load_pins(path);
            if !pins.is_empty() {
                host.set_pins(pins);
            }
        }

        Ok(Self {
            host,
            pins_path,
            settings_path,
            settings,
        })
    }

    pub fn from_host(host: HostCore) -> Self {
        Self {
            host,
            pins_path: None,
            settings_path: None,
            settings: DeckSettings::default(),
        }
    }

    pub fn tick(&mut self) -> anyhow::Result<()> {
        self.tick_at(now_ms())
    }

    pub fn tick_at(&mut self, now: u64) -> anyhow::Result<()> {
        self.host.tick_at(now)?;
        Ok(())
    }

    pub fn board_state(&self) -> BoardState {
        if let Some(b) = self.host.board_state() {
            return b.clone();
        }
        // Should be rare (before first recompute): return empty Off slots.
        let mut board = SessionBoard::new(8);
        board.recompute(now_ms());
        board.board_state().cloned().unwrap()
    }

    pub fn led_frame(&self) -> LedFrame {
        if let Some(l) = self.host.led_frame() {
            return l.clone();
        }
        let mut board = SessionBoard::new(8);
        board.recompute(now_ms());
        board.led_frame().cloned().unwrap()
    }

    pub fn set_focus(&mut self, i: usize) {
        self.host.set_focus(i);
    }

    pub fn set_focus_at(&mut self, i: usize, now: u64) {
        self.host.set_focus_at(i, now);
    }

    /// Pin/unpin a session on a slot (manual bind). Historical catalog sessions are
    /// upserted into the board so they survive the next poll window.
    pub fn pin_slot(&mut self, i: usize, session_id: Option<String>) {
        self.host.pin_session_from_catalog(i, session_id);
        if let Some(ref path) = self.pins_path {
            let _ = save_pins(path, self.host.pins());
        }
    }

    /// Full catalog for bind picker (all projects + history).
    pub fn list_sessions(&mut self) -> Vec<SessionInfo> {
        self.host.list_catalog()
    }

    pub fn settings(&self) -> &DeckSettings {
        &self.settings
    }

    pub fn set_auto_fill(&mut self, enabled: bool) {
        self.settings.auto_fill = enabled;
        self.host.set_auto_fill(enabled);
        if let Some(ref path) = self.settings_path {
            let _ = save_settings(path, &self.settings);
        }
    }

    /// Legacy helper: always false now (demo mode removed from production path).
    pub fn using_demo(&self) -> bool {
        false
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

/// Default settings path: `~/.agent-deck/settings.json`.
pub fn default_settings_path() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".agent-deck/settings.json")
}

pub fn load_settings(path: &Path) -> DeckSettings {
    let Ok(raw) = fs::read_to_string(path) else {
        return DeckSettings::default();
    };
    serde_json::from_str(&raw).unwrap_or_default()
}

pub fn save_settings(path: &Path, settings: &DeckSettings) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let raw = serde_json::to_string_pretty(settings)?;
    fs::write(path, raw)?;
    Ok(())
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
