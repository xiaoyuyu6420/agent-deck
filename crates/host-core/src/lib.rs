//! Host orchestration: poll zcode → board → led/board state.
//! Ported from packages/host/src/main.ts (core loop only)

use agent_deck_board::SessionBoard;
use agent_deck_protocol::{
    home_dir, Action, BackendId, BoardState, DeckStatus, LedFrame, ProjectCategory, SessionSnapshot,
    DONE_TTL_MS, DONE_TTL_UNOPENED_MS,
};
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_category: Option<ProjectCategory>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_label: Option<String>,
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
            project_category: s.project_category,
            project_label: s.project_label.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeckSettings {
    /// When true, unbound slots auto-fill by priority. When false (default),
    /// only manually pinned sessions show — empty keys stay Off.
    #[serde(default)]
    pub auto_fill: bool,
    /// After the user opens a Done key from Agent Deck, keep green this long.
    #[serde(default = "default_done_ttl_after_open_ms")]
    pub done_ttl_after_open_ms: u64,
    /// Unopened Done keys stay green until this max age, then force Idle.
    #[serde(default = "default_done_ttl_unopened_ms")]
    pub done_ttl_unopened_ms: u64,
}

impl Default for DeckSettings {
    fn default() -> Self {
        Self {
            auto_fill: false,
            done_ttl_after_open_ms: default_done_ttl_after_open_ms(),
            done_ttl_unopened_ms: default_done_ttl_unopened_ms(),
        }
    }
}

fn default_done_ttl_after_open_ms() -> u64 {
    DONE_TTL_MS
}

fn default_done_ttl_unopened_ms() -> u64 {
    DONE_TTL_UNOPENED_MS
}

/// Open-aware Done → Idle decay for WorkBuddy (and future backends that opt in).
///
/// - If the user opened the key **after** this Done cycle (`opened_at >= done_since`),
///   the short `after_open_ms` TTL starts at open time.
/// - Otherwise keep Done until `unopened_ms` from `done_since`, then force Idle.
/// - Non-WorkBuddy / non-Done snapshots are returned unchanged.
pub fn decay_done_status(
    snap: &SessionSnapshot,
    opened_at: Option<u64>,
    now: u64,
    after_open_ms: u64,
    unopened_ms: u64,
) -> DeckStatus {
    if snap.backend != BackendId::Workbuddy || snap.status != DeckStatus::Done {
        return snap.status;
    }
    let done_since = snap.updated_at;
    if let Some(opened) = opened_at {
        if opened >= done_since {
            return if now.saturating_sub(opened) > after_open_ms {
                DeckStatus::Idle
            } else {
                DeckStatus::Done
            };
        }
    }
    if now.saturating_sub(done_since) > unopened_ms {
        DeckStatus::Idle
    } else {
        DeckStatus::Done
    }
}

fn opened_key(backend: BackendId, session_id: &str) -> String {
    let prefix = match backend {
        BackendId::Zcode => "zcode",
        BackendId::Codex => "codex",
        BackendId::Workbuddy => "workbuddy",
    };
    format!("{prefix}:{session_id}")
}

fn apply_done_decay(
    snaps: Vec<SessionSnapshot>,
    opened_at: &HashMap<String, u64>,
    now: u64,
    after_open_ms: u64,
    unopened_ms: u64,
) -> Vec<SessionSnapshot> {
    snaps
        .into_iter()
        .map(|mut snap| {
            let opened = opened_at
                .get(&opened_key(snap.backend, &snap.session_id))
                .copied();
            let next = decay_done_status(&snap, opened, now, after_open_ms, unopened_ms);
            if next != snap.status {
                snap.status = next;
            }
            snap
        })
        .collect()
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

    /// Dispatch a write Action to this backend. Returns `Ok(status)` on a
    /// dispatched (or definitively unsupported) action, `Err` only on transport
    /// failure of a *supported* action (the caller treats Err as a transient
    /// error worth retrying).
    ///
    /// Default: the backend cannot perform any write action — return
    /// `Ok("unsupported:{op}")`. Backends override to implement the actions
    /// they support (e.g. Codex Stop via `turn/interrupt`). See
    /// `docs/action-spec.md`.
    fn dispatch(&mut self, action: &Action) -> anyhow::Result<String> {
        Ok(format!("unsupported:{}", action.op_tag()))
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

    fn dispatch(&mut self, action: &Action) -> anyhow::Result<String> {
        Ok(self.dispatch_once(action)?)
    }
}

impl BackendObserver for agent_deck_workbuddy::JsonlObserver {
    fn id(&self) -> BackendId {
        BackendId::Workbuddy
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
    /// When true, also register a WorkBuddy jsonl observer (graceful no-op if
    /// the ~/.workbuddy/projects tree is absent).
    pub enable_workbuddy: bool,
    /// Override path to the WorkBuddy projects tree; None → ~/.workbuddy/projects.
    pub workbuddy_projects_dir: Option<PathBuf>,
}

impl Default for HostConfig {
    fn default() -> Self {
        let home = home_dir();
        Self {
            tasks_db_path: home.join(".zcode/v2/tasks-index.sqlite"),
            tool_db_path: home.join(".zcode/cli/db/db.sqlite"),
            exclude_workspaces: vec![],
            exclude_task_ids: vec![],
            slot_count: 8,
            enable_codex: true,
            codex_cli_path: None,
            enable_workbuddy: true,
            workbuddy_projects_dir: None,
        }
    }
}

pub struct HostCore {
    pub board: SessionBoard,
    observers: Vec<Box<dyn BackendObserver>>,
    /// In-memory "user opened this session from Agent Deck" timestamps.
    /// Keyed as `backend:session_id`. Not persisted across restarts.
    opened_at: HashMap<String, u64>,
    /// Done decay windows (from DeckSettings). Applied only to WorkBuddy Done.
    done_ttl_after_open_ms: u64,
    done_ttl_unopened_ms: u64,
}

impl HostCore {
    pub fn new(config: HostConfig) -> anyhow::Result<Self> {
        let mut observers: Vec<Box<dyn BackendObserver>> = Vec::new();

        let mut zcode = SqliteObserver::new(SqliteObserverOptions {
            tasks_db_path: config.tasks_db_path.clone(),
            tool_db_path: config.tool_db_path.clone(),
            exclude_workspaces: config.exclude_workspaces.clone(),
            exclude_task_ids: config.exclude_task_ids.clone(),
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

        if config.enable_workbuddy {
            let workbuddy_projects_dir = config
                .workbuddy_projects_dir
                .clone()
                .unwrap_or_else(|| {
                    let home = home_dir();
                    home.join(".workbuddy/projects")
                });
            let mut workbuddy =
                agent_deck_workbuddy::JsonlObserver::new(agent_deck_workbuddy::JsonlObserverOptions {
                    projects_dir: workbuddy_projects_dir,
                    exclude_workspaces: config.exclude_workspaces.clone(),
                    exclude_task_ids: config.exclude_task_ids.clone(),
                    ..Default::default()
                });
            // Missing tree is fine — observer stays empty.
            let _ = workbuddy.open();
            observers.push(Box::new(workbuddy));
        }

        let board = SessionBoard::new(config.slot_count);
        Ok(Self {
            board,
            observers,
            opened_at: HashMap::new(),
            done_ttl_after_open_ms: DONE_TTL_MS,
            done_ttl_unopened_ms: DONE_TTL_UNOPENED_MS,
        })
    }

    /// Construct with an explicit observer list (tests inject fixtures).
    pub fn with_observers(slot_count: usize, observers: Vec<Box<dyn BackendObserver>>) -> Self {
        Self {
            board: SessionBoard::new(slot_count),
            observers,
            opened_at: HashMap::new(),
            done_ttl_after_open_ms: DONE_TTL_MS,
            done_ttl_unopened_ms: DONE_TTL_UNOPENED_MS,
        }
    }

    /// Update Done decay windows (from settings). Takes effect on next tick.
    pub fn set_done_ttl(&mut self, after_open_ms: u64, unopened_ms: u64) {
        self.done_ttl_after_open_ms = after_open_ms;
        self.done_ttl_unopened_ms = unopened_ms;
    }

    /// Record that the user opened this session from Agent Deck (key click).
    /// In-memory only; does not require the backend app open to succeed.
    pub fn mark_opened(&mut self, backend: BackendId, session_id: &str) {
        self.mark_opened_at(backend, session_id, now_ms());
    }

    pub fn mark_opened_at(&mut self, backend: BackendId, session_id: &str, now: u64) {
        self.opened_at
            .insert(opened_key(backend, session_id), now);
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
        // Copy decay inputs so we don't fight the mutable borrow of observers.
        let opened_at = self.opened_at.clone();
        let after_open_ms = self.done_ttl_after_open_ms;
        let unopened_ms = self.done_ttl_unopened_ms;

        for obs in &mut self.observers {
            // One backend failing must not block the others.
            match obs.poll() {
                Ok(snaps) => {
                    let snaps =
                        apply_done_decay(snaps, &opened_at, now, after_open_ms, unopened_ms);
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
                    let pinned_snaps = apply_done_decay(
                        pinned_snaps,
                        &opened_at,
                        now,
                        after_open_ms,
                        unopened_ms,
                    );
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
                        let now = now_ms();
                        let snaps = apply_done_decay(
                            vec![snap],
                            &self.opened_at,
                            now,
                            self.done_ttl_after_open_ms,
                            self.done_ttl_unopened_ms,
                        );
                        if let Some(snap) = snaps.into_iter().next() {
                            self.board.upsert_session(snap, now);
                        }
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

    /// Dispatch a write Action. Resolves the target slot (explicit `i`, else
    /// the focused slot), finds its bound backend, and forwards the action to
    /// that backend's observer. Returns the observer's status string.
    ///
    /// Slot-scoped actions (Accept/Reject/Stop) without a resolvable target
    /// return `unsupported:{tag}:no_target`. Non-slot actions (StopAll,
    /// FreezeAll, SetMode) are handled at the DesktopService layer (board-local
    /// state) or broadcast to all observers.
    pub fn dispatch_action(&mut self, action: &Action) -> String {
        let tag = action.op_tag();
        match action {
            // Board-local actions (no backend round-trip). Currently stubbed —
            // Phase 3 will implement FreezeAll/Unfreeze/SetMode here.
            Action::FreezeAll | Action::Unfreeze | Action::SetMode { .. } => {
                format!("unsupported:{tag}")
            }
            // Broadcast: interrupt every backend's running sessions.
            Action::StopAll => {
                let mut last = String::from("ok:stop_all:noop");
                for obs in &mut self.observers {
                    // StopAll has no slot; observers fall back to their first
                    // live thread. A backend with nothing running returns
                    // unsupported and we continue to the next.
                    match obs.dispatch(action) {
                        Ok(s) => last = s,
                        Err(e) => last = format!("error:stop_all:{e}"),
                    }
                }
                last
            }
            // Slot-scoped: Accept/Reject/Stop/Send need a target slot.
            Action::Accept { i }
            | Action::Reject { i }
            | Action::Stop { i }
            | Action::Send { i, .. } => {
                let slot_i = i.or(Some(self.board.focus()));
                let Some(slot_i) = slot_i else {
                    return format!("unsupported:{tag}:no_target");
                };
                let Some(board) = self.board.board_state() else {
                    return format!("unsupported:{tag}:no_board");
                };
                let Some(binding) = board.slots.get(slot_i) else {
                    return format!("unsupported:{tag}:bad_slot");
                };
                let (Some(backend), Some(_session_id)) =
                    (binding.backend, binding.session_id.as_deref())
                else {
                    return format!("unsupported:{tag}:empty_slot");
                };
                // Find the observer for this backend.
                let Some(obs) = self.observers.iter_mut().find(|o| o.id() == backend) else {
                    return format!("unsupported:{tag}:no_observer");
                };
                match obs.dispatch(action) {
                    Ok(s) => s,
                    Err(e) => format!("error:{tag}:{e}"),
                }
            }
            // Focus/Pin are pure-local and already handled by dedicated
            // commands (set_focus / pin_slot); reaching dispatch means misuse.
            Action::Focus { .. } | Action::Pin { .. } => format!("unsupported:{tag}"),
        }
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
        host.set_done_ttl(
            settings.done_ttl_after_open_ms,
            settings.done_ttl_unopened_ms,
        );

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

    /// Record that the user opened this session from Agent Deck (key click).
    /// Starts the short Done TTL for WorkBuddy when the session is Done.
    pub fn mark_opened(&mut self, backend: BackendId, session_id: &str) {
        self.host.mark_opened(backend, session_id);
    }

    pub fn mark_opened_at(&mut self, backend: BackendId, session_id: &str, now: u64) {
        self.host.mark_opened_at(backend, session_id, now);
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

    /// Update open-aware Done TTLs and persist settings.
    pub fn set_done_ttl(&mut self, after_open_ms: u64, unopened_ms: u64) {
        self.settings.done_ttl_after_open_ms = after_open_ms;
        self.settings.done_ttl_unopened_ms = unopened_ms;
        self.host.set_done_ttl(after_open_ms, unopened_ms);
        if let Some(ref path) = self.settings_path {
            let _ = save_settings(path, &self.settings);
        }
    }

    /// Legacy helper: always false now (demo mode removed from production path).
    pub fn using_demo(&self) -> bool {
        false
    }

    /// Dispatch a UI action string ("accept"/"reject"/"stop"/"stop_all").
    ///
    /// Parses the string into an `Action` targeting the **currently focused
    /// slot** (the UI highlights focus before the user hits OK/NO/STP), then
    /// routes it through `HostCore::dispatch_action`. Returns the status string
    /// (`ok:...` / `unsupported:...` / `error:...`) for the UI to surface.
    pub fn dispatch_action(&mut self, action: &str) -> String {
        let parsed = match parse_ui_action(action) {
            Some(a) => a,
            None => return format!("unsupported:unknown:{action}"),
        };
        self.host.dispatch_action(&parsed)
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

/// Parse a UI action string ("accept"/"reject"/"stop"/"stop_all") into an
/// `Action`. Slot index is left `None` — `HostCore::dispatch_action` resolves
/// it to the focused slot. Unknown strings return `None`.
pub fn parse_ui_action(s: &str) -> Option<Action> {
    Some(match s.trim().to_ascii_lowercase().as_str() {
        "accept" | "ok" => Action::Accept { i: None },
        "reject" | "no" => Action::Reject { i: None },
        "stop" | "stp" => Action::Stop { i: None },
        "stop_all" | "stopall" => Action::StopAll,
        _ => return None,
    })
}

/// Default pin persistence path: `~/.agent-deck/pins.json`.
pub fn default_pins_path() -> PathBuf {
    home_dir().join(".agent-deck/pins.json")
}

/// Default settings path: `~/.agent-deck/settings.json`.
pub fn default_settings_path() -> PathBuf {
    home_dir().join(".agent-deck/settings.json")
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
                project_category: None,
                project_label: None,
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
                project_category: None,
                project_label: None,
            },
        ],
        now,
    );
    (
        board.led_frame().cloned().unwrap(),
        board.board_state().cloned().unwrap(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A controllable observer that records every dispatch call. Used to prove
    /// the action router reaches the backend bound to the focused slot (not a
    /// global stub) and that slot index resolution is correct.
    struct RecordingObserver {
        id: BackendId,
        snaps: Vec<SessionSnapshot>,
        dispatched: std::sync::Mutex<Vec<Action>>,
        supported: Vec<&'static str>,
    }

    impl BackendObserver for RecordingObserver {
        fn id(&self) -> BackendId {
            self.id
        }
        fn poll(&mut self) -> anyhow::Result<Vec<SessionSnapshot>> {
            Ok(self.snaps.clone())
        }
        fn dispatch(&mut self, action: &Action) -> anyhow::Result<String> {
            *self.dispatched.lock().unwrap() = vec![action.clone()];
            let tag = action.op_tag();
            if self.supported.iter().any(|s| *s == tag) {
                Ok(format!("ok:{tag}"))
            } else {
                Ok(format!("unsupported:{tag}"))
            }
        }
    }

    fn snap(backend: BackendId, id: &str, status: DeckStatus) -> SessionSnapshot {
        SessionSnapshot {
            backend,
            session_id: id.into(),
            title: id.into(),
            status,
            risk: None,
            detail: None,
            waiting_since: None,
            updated_at: 1000,
            workspace_path: None,
            project_category: None,
            project_label: None,
        }
    }

    #[test]
    fn dispatch_routes_stop_to_focused_slot_backend() {
        // Slot allocator ranks Waiting(5) > Working(3), so:
        //   slot 0 = codex (waiting), slot 1 = zcode (working).
        let zcode_obs = RecordingObserver {
            id: BackendId::Zcode,
            snaps: vec![snap(BackendId::Zcode, "z1", DeckStatus::Working)],
            dispatched: std::sync::Mutex::new(vec![]),
            supported: vec![],
        };
        let codex_obs = RecordingObserver {
            id: BackendId::Codex,
            snaps: vec![snap(BackendId::Codex, "c1", DeckStatus::Waiting)],
            dispatched: std::sync::Mutex::new(vec![]),
            supported: vec!["stop"],
        };
        let mut host = HostCore::with_observers(
            5,
            vec![Box::new(zcode_obs), Box::new(codex_obs)],
        );
        host.tick_at(1000).unwrap();
        // Sanity: confirm the allocation we rely on.
        let board = host.board_state().unwrap();
        assert_eq!(board.slots[0].backend, Some(BackendId::Codex));
        assert_eq!(board.slots[1].backend, Some(BackendId::Zcode));
        // Focus 0 = codex → stop succeeds.
        host.set_focus_at(0, 1000);
        let r = host.dispatch_action(&Action::Stop { i: None });
        assert_eq!(r, "ok:stop", "codex should accept stop, got: {r}");
        // Focus 1 = zcode → stop unsupported (zcode has no write path).
        host.set_focus_at(1, 1000);
        let r = host.dispatch_action(&Action::Stop { i: None });
        assert!(r.starts_with("unsupported:stop"), "got: {r}");
    }

    #[test]
    fn dispatch_empty_slot_returns_no_target() {
        let zcode_obs = RecordingObserver {
            id: BackendId::Zcode,
            snaps: vec![],
            dispatched: std::sync::Mutex::new(vec![]),
            supported: vec!["accept"],
        };
        let mut host = HostCore::with_observers(5, vec![Box::new(zcode_obs)]);
        host.tick_at(1000).unwrap();
        let r = host.dispatch_action(&Action::Accept { i: Some(0) });
        assert!(r.contains("empty_slot") || r.contains("no_target"), "got: {r}");
    }

    #[test]
    fn dispatch_explicit_slot_overrides_focus() {
        // slot 0 = codex (waiting), slot 1 = zcode (working).
        let zcode_obs = RecordingObserver {
            id: BackendId::Zcode,
            snaps: vec![snap(BackendId::Zcode, "z1", DeckStatus::Working)],
            dispatched: std::sync::Mutex::new(vec![]),
            supported: vec!["accept"],
        };
        let codex_obs = RecordingObserver {
            id: BackendId::Codex,
            snaps: vec![snap(BackendId::Codex, "c1", DeckStatus::Waiting)],
            dispatched: std::sync::Mutex::new(vec![]),
            supported: vec![],
        };
        let mut host = HostCore::with_observers(
            5,
            vec![Box::new(zcode_obs), Box::new(codex_obs)],
        );
        host.tick_at(1000).unwrap();
        // Focus is 0 (codex), but we explicitly target slot 1 (zcode), which
        // supports accept → ok. Proves explicit `i` wins over focus.
        host.set_focus_at(0, 1000);
        let r = host.dispatch_action(&Action::Accept { i: Some(1) });
        assert_eq!(r, "ok:accept", "zcode slot 1 should accept, got: {r}");
    }

    #[test]
    fn parse_ui_action_maps_known_strings() {
        assert!(matches!(
            parse_ui_action("accept").unwrap(),
            Action::Accept { i: None }
        ));
        assert!(matches!(
            parse_ui_action("OK").unwrap(),
            Action::Accept { i: None }
        ));
        assert!(matches!(
            parse_ui_action("stp").unwrap(),
            Action::Stop { i: None }
        ));
        assert!(matches!(parse_ui_action("stop_all").unwrap(), Action::StopAll));
        assert!(parse_ui_action("nope").is_none());
    }

    fn wb_done(session_id: &str, done_since: u64) -> SessionSnapshot {
        SessionSnapshot {
            backend: BackendId::Workbuddy,
            session_id: session_id.into(),
            title: "t".into(),
            status: DeckStatus::Done,
            risk: None,
            detail: None,
            waiting_since: None,
            updated_at: done_since,
            workspace_path: None,
                project_category: None,
                project_label: None,
        }
    }

    fn zcode_done(session_id: &str, done_since: u64) -> SessionSnapshot {
        SessionSnapshot {
            backend: BackendId::Zcode,
            session_id: session_id.into(),
            title: "t".into(),
            status: DeckStatus::Done,
            risk: None,
            detail: None,
            waiting_since: None,
            updated_at: done_since,
            workspace_path: None,
                project_category: None,
                project_label: None,
        }
    }

    #[test]
    fn unopened_stays_done_within_long_ttl() {
        let done_since = 1_000_000;
        let now = done_since + 60 * 60 * 1000; // 1h later
        let snap = wb_done("s1", done_since);
        assert_eq!(
            decay_done_status(&snap, None, now, DONE_TTL_MS, DONE_TTL_UNOPENED_MS),
            DeckStatus::Done
        );
    }

    #[test]
    fn unopened_force_idle_after_long_ttl() {
        let done_since = 1_000_000;
        let now = done_since + DONE_TTL_UNOPENED_MS + 1;
        let snap = wb_done("s1", done_since);
        assert_eq!(
            decay_done_status(&snap, None, now, DONE_TTL_MS, DONE_TTL_UNOPENED_MS),
            DeckStatus::Idle
        );
    }

    #[test]
    fn after_open_stays_done_inside_short_ttl() {
        let done_since = 1_000_000;
        let opened = done_since + 10_000;
        let now = opened + DONE_TTL_MS - 1;
        let snap = wb_done("s1", done_since);
        assert_eq!(
            decay_done_status(
                &snap,
                Some(opened),
                now,
                DONE_TTL_MS,
                DONE_TTL_UNOPENED_MS
            ),
            DeckStatus::Done
        );
    }

    #[test]
    fn after_open_idle_past_short_ttl() {
        let done_since = 1_000_000;
        let opened = done_since + 10_000;
        let now = opened + DONE_TTL_MS + 1;
        let snap = wb_done("s1", done_since);
        assert_eq!(
            decay_done_status(
                &snap,
                Some(opened),
                now,
                DONE_TTL_MS,
                DONE_TTL_UNOPENED_MS
            ),
            DeckStatus::Idle
        );
    }

    #[test]
    fn open_before_done_counts_as_unopened() {
        // User opened while Working; Done arrives later → need a fresh open.
        let done_since = 1_000_000;
        let opened = done_since - 5_000;
        let now = done_since + DONE_TTL_MS + 60_000; // past short TTL from open
        let snap = wb_done("s1", done_since);
        assert_eq!(
            decay_done_status(
                &snap,
                Some(opened),
                now,
                DONE_TTL_MS,
                DONE_TTL_UNOPENED_MS
            ),
            DeckStatus::Done
        );
    }

    #[test]
    fn non_workbuddy_done_untouched() {
        let done_since = 1_000_000;
        let now = done_since + DONE_TTL_UNOPENED_MS + 1;
        let snap = zcode_done("s1", done_since);
        assert_eq!(
            decay_done_status(&snap, None, now, DONE_TTL_MS, DONE_TTL_UNOPENED_MS),
            DeckStatus::Done
        );
    }

    #[test]
    fn settings_defaults_fill_missing_ttl_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        // Old settings file only had autoFill.
        fs::write(&path, r#"{"autoFill":true}"#).unwrap();
        let s = load_settings(&path);
        assert!(s.auto_fill);
        assert_eq!(s.done_ttl_after_open_ms, DONE_TTL_MS);
        assert_eq!(s.done_ttl_unopened_ms, DONE_TTL_UNOPENED_MS);
    }
}
