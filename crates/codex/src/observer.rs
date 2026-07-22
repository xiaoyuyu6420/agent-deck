//! Poll codex app-server for thread list → SessionSnapshot.

use crate::mapper::{map_thread, CodexThread};
use crate::rpc::{detect_codex_cli, JsonRpcClient, RpcError};
use agent_deck_protocol::SessionSnapshot;
use serde::Deserialize;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone)]
pub struct CodexObserverOptions {
    pub cli_path: Option<PathBuf>,
    /// Only surface threads updated within this many seconds (filters notLoaded noise).
    pub recency_window_secs: u64,
    /// Max threads to keep after filtering/sorting.
    pub max_threads: usize,
}

impl Default for CodexObserverOptions {
    fn default() -> Self {
        Self {
            cli_path: None,
            recency_window_secs: 24 * 60 * 60,
            max_threads: 20,
        }
    }
}

pub struct CodexObserver {
    opts: CodexObserverOptions,
    client: Option<JsonRpcClient>,
    last_snapshots: Vec<SessionSnapshot>,
    open_failed: bool,
}

impl CodexObserver {
    pub fn new(opts: CodexObserverOptions) -> Self {
        Self {
            opts,
            client: None,
            last_snapshots: vec![],
            open_failed: false,
        }
    }

    pub fn open(&mut self) -> Result<(), RpcError> {
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
        if self.client.is_none() && !self.open_failed {
            if let Err(e) = self.open() {
                self.open_failed = true;
                // Graceful degrade: no codex available → empty, not hard error.
                let _ = e;
                return Ok(vec![]);
            }
        }
        let Some(client) = self.client.as_mut() else {
            return Ok(self.last_snapshots.clone());
        };

        let result: ThreadListResult = match client.request("thread/list", serde_json::json!({})) {
            Ok(r) => r,
            Err(_) => {
                // Drop dead client; next poll will try reconnect once.
                self.client = None;
                return Ok(self.last_snapshots.clone());
            }
        };

        let now_sec = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let window = self.opts.recency_window_secs;

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

        let snaps: Vec<SessionSnapshot> = threads
            .iter()
            .filter(|t| {
                // Always keep active/error; idle/notLoaded only if recent.
                match &t.status {
                    crate::mapper::ThreadStatus::Active { .. }
                    | crate::mapper::ThreadStatus::SystemError => true,
                    _ => {
                        let ua = t.updated_at.or(t.recency_at).unwrap_or(0);
                        now_sec.saturating_sub(ua) <= window
                    }
                }
            })
            .take(self.opts.max_threads)
            .map(map_thread)
            // Idle/notLoaded map to Idle; drop pure Idle so they don't fill slots
            // ahead of zcode working tasks. Keep Error/Working/Waiting/Done.
            .filter(|s| {
                !matches!(
                    s.status,
                    agent_deck_protocol::DeckStatus::Idle | agent_deck_protocol::DeckStatus::Off
                )
            })
            .collect();

        self.last_snapshots = snaps.clone();
        Ok(snaps)
    }

    pub fn last_snapshots(&self) -> &[SessionSnapshot] {
        &self.last_snapshots
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
