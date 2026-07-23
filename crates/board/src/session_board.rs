//! SessionBoard — merges backends, allocates slots, paints LEDs.
//! Ported from packages/host/src/board/SessionBoard.ts

use crate::slot_allocator::{allocate_slots, AllocatedSlot, ScoredSession, SlotAllocatorOptions};
use crate::theme::{paint, ThemeInput, ThemePalette, CODEX_THEME};
use agent_deck_protocol::{
    BackendId, BoardState, DeckStatus, LedFrame, LedSlot, PolicyMode, SessionSnapshot, SlotBinding,
    SLOT_COUNT,
};
use std::collections::HashMap;

pub struct SessionBoard {
    slot_count: usize,
    palette: ThemePalette,
    sessions: HashMap<String, SessionSnapshot>,
    focus: usize,
    pins: HashMap<usize, String>,
    /// When false, only pinned sessions occupy slots (manual bind mode).
    auto_fill: bool,
    mode: PolicyMode,
    last_led: Option<LedFrame>,
    last_board: Option<BoardState>,
}

impl Default for SessionBoard {
    fn default() -> Self {
        Self::new(SLOT_COUNT)
    }
}

impl SessionBoard {
    pub fn new(slot_count: usize) -> Self {
        Self {
            slot_count,
            palette: CODEX_THEME,
            sessions: HashMap::new(),
            focus: 0,
            pins: HashMap::new(),
            // Algorithm default is auto-fill; DesktopService may disable it for
            // manual-bind UX (empty keys until the user pins a session).
            auto_fill: true,
            mode: PolicyMode::Act,
            last_led: None,
            last_board: None,
        }
    }

    pub fn set_auto_fill(&mut self, enabled: bool, now: u64) {
        self.auto_fill = enabled;
        self.recompute(now);
    }

    pub fn auto_fill(&self) -> bool {
        self.auto_fill
    }

    /// All known sessions across backends (board cache; prefer catalog for bind UI).
    pub fn list_sessions(&self) -> Vec<SessionSnapshot> {
        let mut out: Vec<SessionSnapshot> = self.sessions.values().cloned().collect();
        out.sort_by(|a, b| {
            b.status
                .priority()
                .cmp(&a.status.priority())
                .then_with(|| b.updated_at.cmp(&a.updated_at))
        });
        out
    }

    /// Insert/replace a session snapshot without wiping the rest of the backend.
    /// Used when the user binds a historical session from the catalog.
    pub fn upsert_session(&mut self, snapshot: SessionSnapshot, now: u64) {
        let k = Self::key(snapshot.backend, &snapshot.session_id);
        self.sessions.insert(k, snapshot);
        self.recompute(now);
    }

    /// Insert/replace many snapshots at once, recomputing only once at the end.
    /// Used by the pinned-refresh path in `tick_at` to override stale cached
    /// rows with fresh state for bound sessions.
    pub fn upsert_sessions(&mut self, snapshots: Vec<SessionSnapshot>, now: u64) {
        if snapshots.is_empty() {
            return;
        }
        for snap in snapshots {
            let k = Self::key(snap.backend, &snap.session_id);
            self.sessions.insert(k, snap);
        }
        self.recompute(now);
    }

    pub fn find_session(&self, session_id: &str) -> Option<&SessionSnapshot> {
        self.sessions.values().find(|s| s.session_id == session_id)
    }

    fn key(backend: BackendId, session_id: &str) -> String {
        format!(
            "{}:{session_id}",
            match backend {
                BackendId::Zcode => "zcode",
                BackendId::Codex => "codex",
            }
        )
    }

    fn is_pinned_session(&self, session_id: &str) -> bool {
        self.pins.values().any(|id| id == session_id)
    }

    pub fn set_focus(&mut self, i: usize) {
        if i < self.slot_count {
            self.focus = i;
        }
    }

    pub fn focus(&self) -> usize {
        self.focus
    }

    /// Pin a session to slot `i`, or unpin it when `session_id` is None.
    /// Pinned slots reserve their position across recompute even if the session
    /// disappears, so a task stays bound to a fixed key (the "memory point").
    pub fn set_pin(&mut self, i: usize, session_id: Option<String>, now: u64) {
        if i >= self.slot_count {
            return;
        }
        match session_id {
            Some(id) => {
                self.pins.insert(i, id);
            }
            None => {
                self.pins.remove(&i);
            }
        }
        self.recompute(now);
    }

    /// Current pin map (slot index → session id), for persistence.
    pub fn pins(&self) -> &HashMap<usize, String> {
        &self.pins
    }

    /// Replace the whole pin map (e.g. when loading from disk on startup).
    pub fn set_pins(&mut self, pins: HashMap<usize, String>, now: u64) {
        self.pins = pins;
        self.recompute(now);
    }

    pub fn replace_backend_sessions(
        &mut self,
        backend: BackendId,
        snapshots: Vec<SessionSnapshot>,
        now: u64,
    ) {
        let prefix = match backend {
            BackendId::Zcode => "zcode:",
            BackendId::Codex => "codex:",
        };
        // Preserve manually pinned historical sessions even if they fall outside
        // the board poll window (LIMIT 20 / Done TTL / idle filter).
        let pinned_keep: HashMap<String, SessionSnapshot> = self
            .sessions
            .iter()
            .filter(|(k, s)| k.starts_with(prefix) && self.is_pinned_session(&s.session_id))
            .map(|(k, s)| (k.clone(), s.clone()))
            .collect();

        self.sessions.retain(|k, _| !k.starts_with(prefix));
        for s in snapshots {
            let k = Self::key(backend, &s.session_id);
            self.sessions.insert(k, s);
        }
        for (k, s) in pinned_keep {
            self.sessions.entry(k).or_insert(s);
        }
        self.recompute(now);
    }

    pub fn recompute(&mut self, now: u64) {
        // Done sessions are no longer purged by age — users want completed
        // sessions to stay visible. The slot allocator ranks Done lowest, so
        // active sessions always take precedence and Done only fills leftover
        // slots. (Pinned sessions are of course never dropped.)
        let _ = now;
        let pinned_ids: std::collections::HashSet<String> = self.pins.values().cloned().collect();
        self.sessions.retain(|_, s| {
            if pinned_ids.contains(&s.session_id) {
                return true;
            }
            // Only drop sessions that the backend itself stopped reporting
            // (handled by replace_backend_sessions); keep everything we have.
            true
        });

        let scored: Vec<ScoredSession> = self
            .sessions
            .values()
            .map(|s| ScoredSession {
                urgency: compute_urgency(s, now),
                snapshot: s.clone(),
            })
            .collect();

        let opts = SlotAllocatorOptions {
            slot_count: self.slot_count,
            focus: Some(self.focus),
            pins: self.pins.clone(),
        };
        let mut allocated = allocate_slots(&scored, &opts, now);
        // Manual mode: clear non-pinned slots so unbound keys stay Off.
        if !self.auto_fill {
            for slot in &mut allocated {
                if !slot.pinned {
                    slot.session = None;
                }
            }
        }
        let led = self.build_led_frame(&allocated, now);
        let board = self.build_board_state(&allocated);
        self.last_led = Some(led);
        self.last_board = Some(board);
    }

    fn build_led_frame(&self, allocated: &[AllocatedSlot], now: u64) -> LedFrame {
        let slots = allocated
            .iter()
            .map(|slot| {
                if let Some(ref session) = slot.session {
                    let out = paint(
                        &ThemeInput {
                            status: session.snapshot.status,
                            risk: session.snapshot.risk,
                            waiting_since: session.snapshot.waiting_since,
                            now,
                        },
                        &self.palette,
                    );
                    LedSlot {
                        i: slot.i,
                        rgb: out.rgb,
                        br: out.br,
                        fx: out.fx,
                    }
                } else {
                    LedSlot {
                        i: slot.i,
                        rgb: None,
                        br: 0,
                        fx: agent_deck_protocol::LedFx::Solid,
                    }
                }
            })
            .collect();
        LedFrame::new(slots)
    }

    fn build_board_state(&self, allocated: &[AllocatedSlot]) -> BoardState {
        let slots = allocated
            .iter()
            .map(|slot| {
                if let Some(ref session) = slot.session {
                    SlotBinding {
                        i: slot.i,
                        backend: Some(session.snapshot.backend),
                        session_id: Some(session.snapshot.session_id.clone()),
                        title: Some(session.snapshot.title.clone()),
                        status: session.snapshot.status,
                        detail: session.snapshot.detail.clone(),
                        focused: Some(slot.i == self.focus),
                        pinned: Some(slot.pinned),
                    }
                } else {
                    SlotBinding {
                        i: slot.i,
                        backend: None,
                        session_id: None,
                        title: None,
                        status: DeckStatus::Off,
                        detail: None,
                        focused: Some(slot.i == self.focus),
                        pinned: Some(slot.pinned),
                    }
                }
            })
            .collect();
        BoardState::new(slots, self.focus, self.mode)
    }

    pub fn led_frame(&self) -> Option<&LedFrame> {
        self.last_led.as_ref()
    }

    pub fn board_state(&self) -> Option<&BoardState> {
        self.last_board.as_ref()
    }
}

fn compute_urgency(snapshot: &SessionSnapshot, now: u64) -> f64 {
    if snapshot.status != DeckStatus::Waiting {
        return 0.0;
    }
    let Some(waiting_since) = snapshot.waiting_since else {
        return 0.0;
    };
    let age_sec = (now.saturating_sub(waiting_since)) as f64 / 1000.0;
    let time_urgency = (age_sec / 120.0).clamp(0.0, 1.0);
    let risk_boost = snapshot.risk.map(|r| r.boost()).unwrap_or(0.0);
    time_urgency.max(risk_boost)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_pin_reserves_slot_and_marks_board() {
        let mut board = SessionBoard::new(5);
        board.replace_backend_sessions(
            BackendId::Zcode,
            vec![
                SessionSnapshot {
                    backend: BackendId::Zcode,
                    session_id: "a".into(),
                    title: "A".into(),
                    status: DeckStatus::Working,
                    risk: None,
                    detail: None,
                    waiting_since: None,
                    updated_at: 100,
                    workspace_path: None,
                },
                SessionSnapshot {
                    backend: BackendId::Zcode,
                    session_id: "b".into(),
                    title: "B".into(),
                    status: DeckStatus::Done,
                    risk: None,
                    detail: None,
                    waiting_since: None,
                    updated_at: 200,
                    workspace_path: None,
                },
            ],
            1000,
        );
        // Pin the lower-priority "b" into slot 0 — it must stay there.
        board.set_pin(0, Some("b".into()), 1000);
        let state = board.board_state().unwrap();
        assert_eq!(state.slots[0].session_id.as_deref(), Some("b"));
        assert_eq!(state.slots[0].pinned, Some(true));
        assert_eq!(state.slots[1].session_id.as_deref(), Some("a"));
        assert_eq!(state.slots[1].pinned, Some(false));

        // Unpin restores free allocation.
        board.set_pin(0, None, 1000);
        let state = board.board_state().unwrap();
        assert_eq!(state.slots[0].pinned, Some(false));
        assert_eq!(state.slots[0].session_id.as_deref(), Some("a"));
    }

    #[test]
    fn working_session_paints_blue() {
        let mut board = SessionBoard::new(5);
        board.replace_backend_sessions(
            BackendId::Zcode,
            vec![SessionSnapshot {
                backend: BackendId::Zcode,
                session_id: "s1".into(),
                title: "t".into(),
                status: DeckStatus::Working,
                risk: None,
                detail: None,
                waiting_since: None,
                updated_at: 1000,
                workspace_path: None,
            }],
            1000,
        );
        let led = board.led_frame().unwrap();
        let occupied = led.slots.iter().find(|s| s.rgb.is_some()).unwrap();
        assert_eq!(occupied.fx, agent_deck_protocol::LedFx::Breathe);
        let rgb = occupied.rgb.unwrap();
        assert!(rgb[2] > 200);
    }
}
