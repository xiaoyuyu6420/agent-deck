//! Manual verification against the real ~/.workbuddy/projects tree.
//!
//! These tests are `#[ignore]` so CI (which has no ~/.workbuddy) skips them.
//! Run locally with:
//!     cargo test -p agent-deck-workbuddy --test real_sessions -- --ignored --nocapture
//!
//! Purpose: confirm the jsonl observer reads real WorkBuddy task files and
//! surfaces them as SessionSnapshots with sane titles / workspace paths /
//! statuses.

use agent_deck_protocol::BackendId;
use agent_deck_workbuddy::{JsonlObserver, JsonlObserverOptions};
use std::path::PathBuf;

fn home() -> PathBuf {
    agent_deck_protocol::home_dir()
}

fn real_observer() -> JsonlObserver {
    let mut obs = JsonlObserver::new(JsonlObserverOptions {
        projects_dir: home().join(".workbuddy/projects"),
        ..Default::default()
    });
    obs.open().expect("open ~/.workbuddy/projects");
    obs
}

#[test]
#[ignore]
fn real_tree_yields_sessions() {
    let mut obs = real_observer();
    let snaps = obs.poll_once().expect("poll");
    eprintln!("workbuddy sessions found: {}", snaps.len());
    for s in snaps.iter().take(10) {
        eprintln!(
            "  [{:?}] {} | {} | ws={:?}",
            s.status, s.session_id, s.title, s.workspace_path
        );
    }
    assert!(!snaps.is_empty(), "expected real WorkBuddy sessions");
    assert!(
        snaps.iter().all(|s| s.backend == BackendId::Workbuddy),
        "all sessions must carry the Workbuddy backend id"
    );
}

#[test]
#[ignore]
fn real_sessions_have_titles_and_workspaces() {
    let mut obs = real_observer();
    let snaps = obs.poll_once().expect("poll");
    // Every real session must carry a workspace path (cwd is on every event).
    let with_ws = snaps.iter().filter(|s| s.workspace_path.is_some()).count();
    eprintln!("{with_ws}/{} with workspace", snaps.len());
    assert_eq!(
        with_ws,
        snaps.len(),
        "every session should have a workspace"
    );

    // Titles: interactive tasks carry an ai-title; headless `automation-*`
    // sessions legitimately have none. Require that the non-automation subset
    // is mostly titled rather than asserting on the whole population.
    let interactive: Vec<_> = snaps
        .iter()
        .filter(|s| {
            !s.workspace_path
                .as_deref()
                .unwrap_or("")
                .contains("automation-")
        })
        .collect();
    if !interactive.is_empty() {
        let titled = interactive
            .iter()
            .filter(|s| s.title != "(untitled)")
            .count();
        eprintln!("{titled}/{} interactive sessions titled", interactive.len());
        assert!(
            titled as f64 / interactive.len() as f64 > 0.5,
            "most interactive sessions should have a title"
        );
    }
}

#[test]
#[ignore]
fn real_catalog_is_superset_of_poll() {
    let mut obs = real_observer();
    let polled: Vec<_> = obs.poll_once().expect("poll");
    let catalog = obs.catalog_once().expect("catalog");
    // The catalog window is wider, so it must contain every polled id.
    for s in &polled {
        assert!(
            catalog.iter().any(|c| c.session_id == s.session_id),
            "polled session {} missing from catalog",
            s.session_id
        );
    }
    eprintln!("poll={} catalog={}", polled.len(), catalog.len());
    assert!(
        catalog.len() >= polled.len(),
        "catalog should be at least as large as the board poll"
    );
}
