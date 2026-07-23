//! Pure slot allocation algorithm.
//! Ported from packages/host/src/board/slotAllocator.ts

use agent_deck_protocol::{SessionSnapshot, SLOT_COUNT};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone)]
pub struct ScoredSession {
    pub snapshot: SessionSnapshot,
    pub urgency: f64,
}

#[derive(Debug, Clone)]
pub struct SlotAllocatorOptions {
    pub slot_count: usize,
    pub focus: Option<usize>,
    pub pins: HashMap<usize, String>,
}

impl Default for SlotAllocatorOptions {
    fn default() -> Self {
        Self {
            slot_count: SLOT_COUNT,
            focus: None,
            pins: HashMap::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct AllocatedSlot {
    pub i: usize,
    pub session: Option<ScoredSession>,
    pub pinned: bool,
}

fn compare_sessions(a: &ScoredSession, b: &ScoredSession) -> std::cmp::Ordering {
    let pa = a.snapshot.status.priority();
    let pb = b.snapshot.status.priority();
    pb.cmp(&pa)
        .then_with(|| {
            b.urgency
                .partial_cmp(&a.urgency)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .then_with(|| b.snapshot.updated_at.cmp(&a.snapshot.updated_at))
}

pub fn allocate_slots(
    sessions: &[ScoredSession],
    opts: &SlotAllocatorOptions,
    now: u64,
) -> Vec<AllocatedSlot> {
    let slot_count = opts.slot_count;
    let mut slots: Vec<AllocatedSlot> = (0..slot_count)
        .map(|i| AllocatedSlot {
            i,
            session: None,
            pinned: false,
        })
        .collect();

    let mut by_id: HashMap<String, ScoredSession> = HashMap::new();
    for s in sessions {
        by_id.insert(s.snapshot.session_id.clone(), s.clone());
    }

    let mut pinned_session_ids: HashSet<String> = HashSet::new();

    for (slot_i, session_id) in &opts.pins {
        if *slot_i >= slot_count {
            continue;
        }
        slots[*slot_i].pinned = true;
        if let Some(session) = by_id.get(session_id) {
            slots[*slot_i].session = Some(session.clone());
            pinned_session_ids.insert(session_id.clone());
        }
    }

    // Done sessions are NOT purged by age anymore — users want completed
    // sessions to remain visible (green) until displaced by something more
    // active. compare_sessions already ranks Done lowest priority, so active
    // sessions always take precedence and Done only fills leftover slots.
    let mut remaining: Vec<ScoredSession> = by_id
        .into_values()
        .filter(|s| !pinned_session_ids.contains(&s.snapshot.session_id))
        .collect();
    remaining.sort_by(compare_sessions);

    let mut cursor = 0usize;
    for s in remaining {
        while cursor < slot_count && (slots[cursor].session.is_some() || slots[cursor].pinned) {
            cursor += 1;
        }
        if cursor >= slot_count {
            break;
        }
        slots[cursor].session = Some(s);
        cursor += 1;
    }

    let _ = opts.focus;
    // `now` is accepted for API stability / future time-based scoring but is
    // not currently used (age-based done purging was removed).
    let _ = now;
    slots
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_deck_protocol::{BackendId, DeckStatus};

    fn snap(id: &str, status: DeckStatus, updated_at: u64) -> ScoredSession {
        ScoredSession {
            snapshot: SessionSnapshot {
                backend: BackendId::Zcode,
                session_id: id.into(),
                title: id.into(),
                status,
                risk: None,
                detail: None,
                waiting_since: None,
                updated_at,
                workspace_path: None,
            },
            urgency: if status == DeckStatus::Waiting {
                0.5
            } else {
                0.0
            },
        }
    }

    #[test]
    fn waiting_takes_first_slot() {
        let sessions = vec![
            snap("w", DeckStatus::Working, 100),
            snap("wait", DeckStatus::Waiting, 100),
        ];
        let slots = allocate_slots(&sessions, &SlotAllocatorOptions::default(), 1000);
        assert_eq!(
            slots[0].session.as_ref().unwrap().snapshot.session_id,
            "wait"
        );
    }

    #[test]
    fn done_is_kept_but_lowest_priority() {
        // Done sessions are no longer purged by age — they stay visible. But
        // they rank below any active session, so an active one displaces them
        // to a later slot.
        let sessions = vec![
            snap("active", DeckStatus::Working, 100),
            snap("old_done", DeckStatus::Done, 0),
        ];
        let slots = allocate_slots(&sessions, &SlotAllocatorOptions::default(), 1000);
        assert_eq!(
            slots[0].session.as_ref().unwrap().snapshot.session_id,
            "active"
        );
        assert_eq!(
            slots[1].session.as_ref().unwrap().snapshot.session_id,
            "old_done"
        );
    }

    #[test]
    fn done_alone_still_shown() {
        // A lone Done session (even very old) must still appear — it is not
        // auto-purged anymore.
        let sessions = vec![snap("old", DeckStatus::Done, 0)];
        let slots = allocate_slots(&sessions, &SlotAllocatorOptions::default(), 9_999_999);
        assert_eq!(
            slots[0].session.as_ref().unwrap().snapshot.session_id,
            "old"
        );
    }

    #[test]
    fn pin_reserves_slot() {
        let sessions = vec![snap("a", DeckStatus::Working, 100)];
        let mut opts = SlotAllocatorOptions::default();
        opts.pins.insert(0, "missing".into());
        let slots = allocate_slots(&sessions, &opts, 1000);
        assert!(slots[0].pinned);
        assert!(slots[0].session.is_none());
        assert_eq!(slots[1].session.as_ref().unwrap().snapshot.session_id, "a");
    }
}
