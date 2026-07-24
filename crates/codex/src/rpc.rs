//! Minimal line-delimited JSON-RPC 2.0 client over a child process stdio.
//! Verified against `codex app-server --listen stdio://` (codex-cli 0.145).

use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum RpcError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("rpc error: {0}")]
    Remote(String),
    #[error("timeout waiting for response id={0}")]
    Timeout(u64),
    #[error("codex CLI not found")]
    CliNotFound,
    #[error("process exited")]
    ProcessExited,
}

/// Candidate paths for the codex binary.
///
/// macOS: ChatGPT.app embeds it off-PATH. Linux: Homebrew/manual install.
/// Windows: ChatGPT desktop install or npm global (codex.exe / codex.cmd).
pub fn detect_codex_cli() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("CODEX_CLI_PATH") {
        let pb = PathBuf::from(p);
        if pb.is_file() {
            return Some(pb);
        }
    }
    let candidates: Vec<PathBuf> = if cfg!(target_os = "macos") {
        vec![
            PathBuf::from("/Applications/ChatGPT.app/Contents/Resources/codex"),
            PathBuf::from("/usr/local/bin/codex"),
            PathBuf::from("/opt/homebrew/bin/codex"),
        ]
    } else if cfg!(target_os = "windows") {
        let mut cands = vec![
            // Common ChatGPT desktop install dirs.
            PathBuf::from(r"C:\Program Files\ChatGPT\resources\codex.exe"),
            PathBuf::from(r"C:\Program Files\ChatGPT\resources\codex.cmd"),
        ];
        // %LOCALAPPDATA%\Programs\ChatGPT\...
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            cands.push(PathBuf::from(&local).join(r"Programs\ChatGPT\resources\codex.exe"));
        }
        // npm global bin (e.g. %APPDATA%\npm).
        if let Some(appdata) = std::env::var_os("APPDATA") {
            cands.push(PathBuf::from(&appdata).join(r"npm\codex.cmd"));
        }
        cands
    } else {
        // Linux / other Unix.
        vec![
            PathBuf::from("/usr/local/bin/codex"),
            PathBuf::from("/usr/bin/codex"),
        ]
    };
    for c in candidates {
        if c.is_file() {
            return Some(c);
        }
    }
    // Fall back to PATH lookup (adds .exe on Windows automatically).
    let exe_name = if cfg!(target_os = "windows") {
        "codex.exe"
    } else {
        "codex"
    };
    which(exe_name)
}

fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

#[derive(Debug, Serialize)]
struct RpcRequest<'a, P: Serialize> {
    jsonrpc: &'static str,
    id: u64,
    method: &'a str,
    params: P,
}

#[derive(Debug, Serialize)]
struct RpcNotification<'a, P: Serialize> {
    jsonrpc: &'static str,
    method: &'a str,
    params: P,
}

#[derive(Debug, Deserialize)]
struct RpcResponse {
    #[serde(default)]
    id: Option<u64>,
    #[serde(default)]
    result: Option<Value>,
    #[serde(default)]
    error: Option<Value>,
    #[serde(default)]
    method: Option<String>,
}

pub struct JsonRpcClient {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

impl JsonRpcClient {
    pub fn spawn(cli: &Path) -> Result<Self, RpcError> {
        let mut child = Command::new(cli)
            .args(["app-server", "--listen", "stdio://"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let stdin = child.stdin.take().ok_or(RpcError::ProcessExited)?;
        let stdout = child.stdout.take().ok_or(RpcError::ProcessExited)?;
        Ok(Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            next_id: 1,
        })
    }

    pub fn initialize(&mut self) -> Result<(), RpcError> {
        // InitializeParams (codex-cli 0.145.0-alpha.27, verified 2026-07-23 via
        // `codex app-server generate-json-schema`) requires only `clientInfo`;
        // `protocolVersion` is not a field (server tolerates extras, but we keep
        // it clean). An optional `capabilities` object is also accepted.
        let params = serde_json::json!({
            "clientInfo": { "name": "agent-deck", "version": "0.1.0" }
        });
        let _result: Value = self.request("initialize", params)?;
        // Client must notify initialized before further requests.
        self.notify("notifications/initialized", serde_json::json!({}))?;
        Ok(())
    }

    pub fn request<P: Serialize, R: DeserializeOwned>(
        &mut self,
        method: &str,
        params: P,
    ) -> Result<R, RpcError> {
        let id = self.next_id;
        self.next_id += 1;
        let req = RpcRequest {
            jsonrpc: "2.0",
            id,
            method,
            params,
        };
        let line = serde_json::to_string(&req)?;
        writeln!(self.stdin, "{line}")?;
        self.stdin.flush()?;

        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if Instant::now() > deadline {
                return Err(RpcError::Timeout(id));
            }
            let mut buf = String::new();
            let n = self.stdout.read_line(&mut buf)?;
            if n == 0 {
                return Err(RpcError::ProcessExited);
            }
            let trimmed = buf.trim();
            if trimmed.is_empty() {
                continue;
            }
            // Skip server notifications/events; wait for our id.
            let msg: RpcResponse = match serde_json::from_str(trimmed) {
                Ok(m) => m,
                Err(_) => continue,
            };
            if msg.method.is_some() {
                continue;
            }
            if msg.id != Some(id) {
                continue;
            }
            if let Some(err) = msg.error {
                return Err(RpcError::Remote(err.to_string()));
            }
            let result = msg.result.unwrap_or(Value::Null);
            return Ok(serde_json::from_value(result)?);
        }
    }

    pub fn notify<P: Serialize>(&mut self, method: &str, params: P) -> Result<(), RpcError> {
        let notif = RpcNotification {
            jsonrpc: "2.0",
            method,
            params,
        };
        let line = serde_json::to_string(&notif)?;
        writeln!(self.stdin, "{line}")?;
        self.stdin.flush()?;
        Ok(())
    }
}

impl Drop for JsonRpcClient {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
