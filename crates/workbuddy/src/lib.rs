//! WorkBuddy backend: read-only jsonl observer + status mapper.
//!
//! WorkBuddy stores each task/session as a newline-delimited JSON event
//! stream at `~/.workbuddy/projects/<workspace>/<session-id>.jsonl`. Each
//! file is an append-only log of agent events (message / function_call /
//! reasoning / ai-title). This observer scans those files read-only and
//! maps them to `SessionSnapshot`s, mirroring how the zcode observer reads
//! its sqlite.
//!
//! Status is multi-signal (pending tools, assistant `incomplete`,
//! user-awaiting-reply) with recency windows — see
//! `docs/workbuddy-integration.md`.

mod db_meta;
mod mapper;
mod observer;

pub use db_meta::{
    classify_workspace, is_archived, is_claw_workspace, load_automation_names,
    load_deleted_session_ids, load_session_meta, preferred_title, SessionMeta,
};
pub use mapper::{infer_status, map_session, SessionEvent, SessionSignals};
pub use observer::{JsonlObserver, JsonlObserverOptions, ObserverError};
