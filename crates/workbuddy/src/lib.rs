//! WorkBuddy backend: read-only jsonl observer + status mapper.
//!
//! WorkBuddy (Tencent CodeBuddy desktop) stores each task/session as a
//! newline-delimited JSON event stream at
//! `~/.workbuddy/projects/<workspace>/<session-id>.jsonl`. Each file is an
//! append-only log of agent events (message / function_call / reasoning /
//! ai-title). This observer scans those files read-only and maps them to
//! `SessionSnapshot`s, mirroring how the zcode observer reads its sqlite.
//!
//! See `docs/workbuddy-integration.md` for the reverse-engineered format and
//! the limitations (no external control API; observation-only for now).

mod mapper;
mod observer;

pub use mapper::{infer_status, map_session, SessionEvent, SessionSignals};
pub use observer::{JsonlObserver, JsonlObserverOptions, ObserverError};
