//! Codex backend: JSON-RPC observer over `codex app-server --listen stdio://`.
//!
//! Protocol is line-delimited JSON-RPC 2.0 (verified against codex-cli 0.145).
//! Real-time working/waiting for GUI live threads comes from `IpcStateWatcher`:
//! connect to `~/.codex/ipc/ipc.sock`, **announce stream follower**, parse
//! `thread-stream-state-changed` snapshot/patches
//! (`conversationState.threadRuntimeStatus`). See `ipc` module and
//! `docs/codex-integration.md`.

mod ipc;
mod mapper;
mod observer;
mod rpc;

pub use ipc::{default_socket_path, IpcStateWatcher};
pub use mapper::{map_status, map_thread, CodexThread, ThreadStatus};
pub use observer::{CodexObserver, CodexObserverOptions};
pub use rpc::{detect_codex_cli, JsonRpcClient};
