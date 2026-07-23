//! Poll codex app-server for thread list → SessionSnapshot.

use crate::ipc::{default_socket_path, IpcStateWatcher};
use crate::mapper::{map_status, map_thread, CodexThread};
use crate::rpc::{detect_codex_cli, JsonRpcClient, RpcError};
use agent_deck_protocol::{DeckStatus, SessionSnapshot};
use serde::Deserialize;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone)]
pub struct CodexObserverOptions {
    pub cli_path: Option<PathBuf>,
    /// Only surface threads updated within this many seconds (filters notLoaded noise).
    pub recency_window_secs: u64,
    /// Max threads to keep after filtering/sorting (board poll).
    pub max_threads: usize,
    /// Max threads returned by the bind-picker catalog.
    pub catalog_max_threads: usize,
    /// Recency window for catalog idle/history threads (seconds).
    pub catalog_recency_window_secs: u64,
    /// Connect to the ChatGPT.app GUI IpcRouter (`~/.codex/ipc/ipc.sock`) to
    /// observe real-time thread status (working/waiting). When the GUI isn't
    /// running, the watcher stays disconnected and `thread/list`'s static
    /// `notLoaded` status is used as-is (i.e. the pre-ipc behaviour).
    ///
    /// Defaults to `true` on Unix (macOS/Linux); `false` on Windows where the
    /// Unix domain socket channel is unavailable.
    pub enable_ipc: bool,
}

impl Default for CodexObserverOptions {
    fn default() -> Self {
        Self {
            cli_path: None,
            recency_window_secs: 24 * 60 * 60,
            max_threads: 20,
            catalog_max_threads: 200,
            // Keep a much longer history window for manual bind.
            catalog_recency_window_secs: 90 * 24 * 60 * 60,
            enable_ipc: cfg!(unix),
        }
    }
}

pub struct CodexObserver {
    opts: CodexObserverOptions,
    client: Option<JsonRpcClient>,
    /// Real-time status watcher for the ChatGPT.app GUI (best-effort; None when
    /// disabled or not yet started).
    ipc: Option<Arc<IpcStateWatcher>>,
    last_snapshots: Vec<SessionSnapshot>,
    open_failed: bool,
}

impl CodexObserver {
    pub fn new(opts: CodexObserverOptions) -> Self {
        Self {
            opts,
            client: None,
            ipc: None,
            last_snapshots: vec![],
            open_failed: false,
        }
    }

    pub fn open(&mut self) -> Result<(), RpcError> {
        // Best-effort: start the ipc watcher alongside the app-server client.
        // The watcher runs in a background thread and reconnects on its own; a
        // missing socket (GUI not running) is a normal degraded state, not an
        // error.
        if self.ipc.is_none() && self.opts.enable_ipc {
            self.ipc = Some(Arc::new(IpcStateWatcher::spawn(default_socket_path())));
        }

        if self.client.is_some() {
            return Ok(());
        }
        let cli = self
            .opts
            .cli_path
            .clone()
            .or_else(detect_codex_cli)
            .ok_or(RpcError::CliNotFound)?;
        let mut client = JsonRpcClient::spawn(&cli)?;
        client.initialize()?;
        self.client = Some(client);
        self.open_failed = false;
        Ok(())
    }

    pub fn poll_once(&mut self) -> Result<Vec<SessionSnapshot>, RpcError> {
        let snaps = self.fetch_threads(
            self.opts.recency_window_secs,
            self.opts.max_threads,
            /*keep_idle=*/ false,
        )?;
        self.last_snapshots = snaps.clone();
        Ok(snaps)
    }

    /// Full-ish catalog for bind picker: keep idle/history threads in a long window.
    pub fn catalog_once(&mut self) -> Result<Vec<SessionSnapshot>, RpcError> {
        self.fetch_threads(
            self.opts.catalog_recency_window_secs,
            self.opts.catalog_max_threads,
            /*keep_idle=*/ true,
        )
    }

    /// Latest state of pinned threads by id. The codex app-server has no
    /// per-id query, so we pull the long-window catalog (no status filter,
    /// no board-poll truncation) and filter by id client-side. Threads not
    /// present in the result are simply absent (caller keeps last-known).
    pub fn poll_pinned_once(&mut self, ids: &[String]) -> Result<Vec<SessionSnapshot>, RpcError> {
        if ids.is_empty() {
            return Ok(vec![]);
        }
        let snaps = self.fetch_threads(
            self.opts.catalog_recency_window_secs,
            self.opts.catalog_max_threads,
            /*keep_idle=*/ true,
        )?;
        Ok(snaps
            .into_iter()
            .filter(|s| ids.iter().any(|id| id == &s.session_id))
            .collect())
    }

    pub fn last_snapshots(&self) -> &[SessionSnapshot] {
        &self.last_snapshots
    }

    fn fetch_threads(
        &mut self,
        recency_window_secs: u64,
        max_threads: usize,
        keep_idle: bool,
    ) -> Result<Vec<SessionSnapshot>, RpcError> {
        if self.client.is_none() && !self.open_failed {
            if let Err(e) = self.open() {
                self.open_failed = true;
                // Graceful degrade: no codex available → empty, not hard error.
                let _ = e;
                return Ok(vec![]);
            }
        }
        let Some(client) = self.client.as_mut() else {
            return Ok(if keep_idle {
                vec![]
            } else {
                self.last_snapshots.clone()
            });
        };

        // Explicitly request non-archived threads. Schema: archived=false|null
        // returns only non-archived; omit would also default that way on some
        // builds, but pass false so archived never leak into bind picker.
        let result: ThreadListResult = match client.request(
            "thread/list",
            serde_json::json!({ "archived": false }),
        ) {
            Ok(r) => r,
            Err(_) => {
                // Drop dead client; next poll will try reconnect once.
                self.client = None;
                return Ok(if keep_idle {
                    vec![]
                } else {
                    self.last_snapshots.clone()
                });
            }
        };

        let now_sec = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        let mut threads = result.data;
        // Prefer active/waiting, then recent updates.
        threads.sort_by(|a, b| {
            let pa = status_rank(&a.status);
            let pb = status_rank(&b.status);
            pb.cmp(&pa).then_with(|| {
                let ua = a.updated_at.or(a.recency_at).unwrap_or(0);
                let ub = b.updated_at.or(b.recency_at).unwrap_or(0);
                ub.cmp(&ua)
            })
        });

        // Map threads → snapshots, overlaying real-time status from the ipc
        // watcher when available. The overlay runs BEFORE the idle filter so a
        // thread that is notLoaded in thread/list but actually working in the
        // GUI (seen via ipc) is not dropped from the board.
        let ipc = self.ipc.clone();
        let snaps: Vec<SessionSnapshot> = threads
            .iter()
            .filter(|t| {
                // Always keep active/error; idle/notLoaded only if recent enough.
                match &t.status {
                    crate::mapper::ThreadStatus::Active { .. }
                    | crate::mapper::ThreadStatus::SystemError => true,
                    _ => {
                        let ua = t.updated_at.or(t.recency_at).unwrap_or(0);
                        now_sec.saturating_sub(ua) <= recency_window_secs
                    }
                }
            })
            .take(max_threads)
            .map(|t| {
                let mut s = map_thread(t);
                if let Some(ref watcher) = ipc {
                    if let Some(live) = watcher.status_of(&s.session_id) {
                        let mapped = map_status(&live);
                        // Only overlay non-idle real-time states: a working/waiting
                        // thread must surface even though thread/list said
                        // notLoaded. An idle live-state would lose information
                        // relative to the (possibly just-completed) map result,
                        // so keep the mapped status in that case.
                        if mapped != DeckStatus::Idle {
                            s.status = mapped;
                            s.waiting_since = if s.status == DeckStatus::Waiting {
                                Some(s.updated_at)
                            } else {
                                None
                            };
                        }
                    }
                }
                s
            })
            .filter(|s| {
                if keep_idle {
                    true
                } else {
                    // Board poll: drop pure Idle so they don't fill slots.
                    !matches!(s.status, DeckStatus::Idle | DeckStatus::Off)
                }
            })
            .collect();

        Ok(snaps)
    }
}

#[derive(Debug, Deserialize)]
struct ThreadListResult {
    #[serde(default)]
    data: Vec<CodexThread>,
}

fn status_rank(s: &crate::mapper::ThreadStatus) -> u8 {
    use crate::mapper::ThreadStatus::*;
    match s {
        Active { active_flags } => {
            if active_flags
                .iter()
                .any(|f| f == "waitingOnApproval" || f == "waitingOnUserInput")
            {
                5
            } else {
                4
            }
        }
        SystemError => 3,
        Idle => 2,
        NotLoaded => 1,
    }
}
