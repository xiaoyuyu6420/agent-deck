//! Codex backend: JSON-RPC observer over `codex app-server --listen stdio://`.
//!
//! Protocol is line-delimited JSON-RPC 2.0 (verified against codex-cli 0.145).
//! Real-time status for historical threads is `notLoaded`; active turns show
//! `active` / `waitingOnApproval` / `waitingOnUserInput` when the daemon has
//! them loaded. Missing CLI or failed open degrades to empty snapshots.

mod mapper;
mod observer;
mod rpc;

pub use mapper::{map_thread, CodexThread, ThreadStatus};
pub use observer::{CodexObserver, CodexObserverOptions};
pub use rpc::{detect_codex_cli, JsonRpcClient};
