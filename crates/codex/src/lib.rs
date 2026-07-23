//! Codex backend: JSON-RPC observer over `codex app-server --listen stdio://`.
//!
//! Protocol is line-delimited JSON-RPC 2.0 (verified against codex-cli 0.145).
//! Real-time status for historical threads is `notLoaded`; active turns show
//! `active` / `waitingOnApproval` / `waitingOnUserInput` when the daemon has
//! them loaded. Missing CLI or failed open degrades to empty snapshots.
//!
//! 独立 spawn 的 app-server 看不到 GUI 的 live thread 状态（进程内存隔离），
//! 所以 `thread/list` 全是 `notLoaded`。实时 working/waiting 状态通过
//! `IpcStateWatcher` 连 GUI 的 IpcRouter（`~/.codex/ipc/ipc.sock`）订阅
//! `thread-stream-state-changed` 广播获得。详见 `ipc` 模块。

mod ipc;
mod mapper;
mod observer;
mod rpc;

pub use ipc::{default_socket_path, IpcStateWatcher};
pub use mapper::{map_status, map_thread, CodexThread, ThreadStatus};
pub use observer::{CodexObserver, CodexObserverOptions};
pub use rpc::{detect_codex_cli, JsonRpcClient};
