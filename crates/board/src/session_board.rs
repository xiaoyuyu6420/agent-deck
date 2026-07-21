//! SessionBoard — merges backends, allocates slots, paints LEDs.
//! Ported from packages/host/src/board/SessionBoard.ts

use crate::slot_allocator::{allocate_slots, AllocatedSlot, ScoredSession, SlotAllocatorOptions};
use crate::theme::{paint, ThemeInput, ThemePalette, CODEX_THEME};
use agent_deck_protocol::{
    BackendId, BoardState, DeckStatus, LedFrame, LedSlot, PolicyMode, SessionSnapshot, SlotBinding,
    DONE_TTL_MS, SLOT_COUNT,
};
use std::collections::HashMap;

pub struct SessionBoard {
    slot_count: usize,
    palette: ThemePalette,
    sessions: HashMap<String, SessionSnapshot>,
    focus: usize,
    pins: HashMap<usize, String>,
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
            mode: PolicyMode::Act,
            last_led: None,
            last_board: None,
        }
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

    pub fn set_focus(&mut self, i: usize) {
        if i < self.slot_count {
            self.focus = i;
        }
    }

    pub fn focus(&self) -> usize {
        self.focus
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
        self.sessions.retain(|k, _| !k.starts_with(prefix));
        for s in snapshots {
            let k = Self::key(backend, &s.session_id);
            self.sessions.insert(k, s);
        }
        self.recompute(now);
    }

    pub fn recompute(&mut self, now: u64) {
        // purge expired done
        self.sessions.retain(|_, s| {
            !(s.status == DeckStatus::Done && now.saturating_sub(s.updated_at) > DONE_TTL_MS)
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
        let allocated = allocate_slots(&scored, &opts, now);
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
