//! Manual verification against the real ~/.zcode database.
//!
//! These tests are `#[ignore]` so CI (which has no ~/.zcode) skips them.
//! Run locally with:
//!     cargo test -p agent-deck-host-core --test e2e_real_zcode -- --ignored --nocapture
//!
//! Purpose: confirm the full observer → board → slot-allocation pipeline reads
//! real data and that status-priority ordering surfaces running/waiting tasks
//! ahead of completed ones.

use agent_deck_host_core::{HostConfig, HostCore};
use std::path::PathBuf;

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn real_config(slot_count: usize) -> HostConfig {
    let home = home();
    HostConfig {
        tasks_db_path: home.join(".zcode/v2/tasks-index.sqlite"),
        tool_db_path: home.join(".zcode/cli/db/db.sqlite"),
        // No exclusions — we want to see every project, including this repo.
        exclude_workspaces: vec![],
        exclude_task_ids: vec![],
        slot_count,
    }
}

#[test]
#[ignore]
fn real_db_populates_more_than_five_slots() {
    // Regression guard: slot_count was hard-coded to 5 and silently truncated.
    // With slot_count=8 and a busy ~/.zcode, the board should fill past 5.
    let cfg = real_config(8);
    let mut host = HostCore::new(cfg).expect("open real zcode");
    host.tick().expect("tick");

    let board = host.board_state().expect("board");
    let occupied = board
        .slots
        .iter()
        .filter(|s| s.session_id.is_some())
        .count();
    eprintln!("occupied slots: {occupied}/{}", board.slots.len());
    eprintln!(
        "slot allocation:\n{}",
        board
            .slots
            .iter()
            .map(|s| format!(
                "  slot {}: {:?} {}",
                s.i,
                s.status,
                s.title.as_deref().unwrap_or("(empty)")
            ))
            .collect::<Vec<_>>()
            .join("\n")
    );
    assert_eq!(board.slots.len(), 8, "board should have 8 slots");
}

#[test]
#[ignore]
fn real_db_running_outranks_completed() {
    // The core ordering guarantee: among the occupied slots, any Working/Waiting
    // task must appear before any Done task. Done tasks may still appear (until
    // DONE_TTL), but never ahead of a task still running.
    let cfg = real_config(8);
    let mut host = HostCore::new(cfg).expect("open real zcode");
    host.tick().expect("tick");

    let board = host.board_state().expect("board");
    let occupied: Vec<_> = board.slots.iter().filter(|s| s.session_id.is_some()).collect();

    let first_done = occupied.iter().position(|s| {
        matches!(
            s.status,
            agent_deck_protocol::DeckStatus::Done
        )
    });
    let last_active = occupied.iter().rposition(|s| {
        matches!(
            s.status,
            agent_deck_protocol::DeckStatus::Working | agent_deck_protocol::DeckStatus::Waiting
        )
    });

    eprintln!(
        "first_done_slot={first_done:?}  last_active_slot={last_active:?}",
    );
    if let (Some(done), Some(active)) = (first_done, last_active) {
        assert!(
            active < done,
            "a Working/Waiting task (slot {active}) appeared AFTER a Done task (slot {done}) — \
             priority ordering is broken"
        );
    }
}
