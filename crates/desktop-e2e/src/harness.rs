//! Test harness: spin up an isolated data fixture, launch the Agent Deck app,
//! and drive its WKWebView via the macOS Accessibility API through a small
//! swift subprocess (ax_driver.swift).
//!
//! tauri-driver v2 does NOT support macOS (it gates on linux/windows only),
//! and macOS has no desktop WebDriver client that can reach an app-embedded
//! WKWebView. The Accessibility API is the supported native path: WKWebView
//! exposes its DOM tree as an AX tree where every element carries
//! AXDOMIdentifier (the DOM id) and AXDOMClassList (the DOM class), so we can
//! locate elements with CSS-like selectors. Pressing an AXButton fires the
//! same click path a real user does.
//!
//! Data isolation: the app's `default_config()` reads
//! AGENT_DECK_TASKS_DB / AGENT_DECK_TOOL_DB (see apps/desktop/src-tauri/src/lib.rs).
//! We set them to the fixture's temp paths before launching the app.

use crate::fixture::Fixture;
use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// The swift driver lives next to this crate's Cargo.toml.
const AX_DRIVER_SRC: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/ax_driver.swift");

/// A JSON-RPC client over the swift driver's stdin/stdout. Owns the driver
/// child; dropping closes stdin (causing the driver's read loop to exit) and
/// reaps the process.
struct AxClient {
    stdin: Option<Box<dyn Write + Send>>,
    stdout: BufReader<std::process::ChildStdout>,
    child: Option<Child>,
    next_id: i64,
}

impl Drop for AxClient {
    fn drop(&mut self) {
        // Close stdin first so the swift read loop returns and the process exits.
        self.stdin.take();
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl AxClient {
    fn spawn() -> Result<Self> {
        let mut child = Command::new("swift")
            .arg(AX_DRIVER_SRC)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .context("failed to spawn `swift` — is Xcode/swift toolchain installed?")?;
        let stdin: Box<dyn Write + Send> = Box::new(child.stdin.take().unwrap());
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Ok(Self {
            stdin: Some(stdin),
            stdout,
            child: Some(child),
            next_id: 1,
        })
    }

    /// Send a request and await its matching response (by id).
    fn call(&mut self, op: &str, extra: Value) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let mut req = json!({ "id": id, "op": op });
        if let Value::Object(map) = &mut req {
            if let Value::Object(extra_map) = extra {
                for (k, v) in extra_map {
                    map.insert(k, v);
                }
            }
        }
        let line = serde_json::to_string(&req)? + "\n";
        let stdin = self
            .stdin
            .as_mut()
            .context("swift driver stdin already closed")?;
        stdin
            .write_all(line.as_bytes())
            .context("write to swift driver failed")?;
        stdin.flush()?;

        // Read lines until we see our id (swift may emit stray output).
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if Instant::now() > deadline {
                bail!("timeout waiting for swift driver response to op={op}");
            }
            let mut buf = String::new();
            let n = self
                .stdout
                .read_line(&mut buf)
                .context("swift driver stdout closed")?;
            if n == 0 {
                bail!("swift driver exited unexpectedly");
            }
            let trimmed = buf.trim();
            if trimmed.is_empty() || !trimmed.starts_with('{') {
                continue; // skip non-JSON lines
            }
            let resp: Value = match serde_json::from_str(trimmed) {
                Ok(v) => v,
                Err(_) => continue,
            };
            if resp.get("id").and_then(|v| v.as_i64()) == Some(id) {
                return Ok(resp);
            }
        }
    }
}

/// Resolve the app bundle path.
fn resolve_app_path() -> Result<PathBuf> {
    if let Ok(p) = std::env::var("AGENT_DECK_APP") {
        let p = PathBuf::from(p);
        if p.exists() {
            return Ok(p);
        }
        return Err(anyhow!(
            "AGENT_DECK_APP set but path does not exist: {}",
            p.display()
        ));
    }
    let app = PathBuf::from("target/release/bundle/macos/Agent Deck.app");
    if app.exists() {
        return Ok(app);
    }
    Err(anyhow!(
        "App bundle not found at {}. Run `pnpm build:desktop` (cargo tauri build) first, \
         or set AGENT_DECK_APP to an existing .app path.",
        app.display()
    ))
}

/// A running e2e session: the AX driver client and the fixture guard. The app
/// is launched detached (via `open`), so we don't hold a Child for it — Drop
/// quits it via osascript. The swift driver child is owned by `ax` and torn
/// down by `AxClient::Drop`.
pub struct E2e {
    ax: AxClient,
    _fixture: Fixture,
}

impl E2e {
    pub fn start() -> Result<Self> {
        let app_path = resolve_app_path()?;

        let fixture = Fixture::empty()?;
        // Data isolation: set before launching the app so it picks them up.
        std::env::set_var("AGENT_DECK_TASKS_DB", &fixture.tasks_db);
        std::env::set_var("AGENT_DECK_TOOL_DB", &fixture.tool_db);

        // Launch the app via `open` (blessed macOS launcher; carries env).
        // Pass the full .app path as a positional arg (NOT -a, which expects an
        // app *name* looked up in /Applications and rejects a path).
        let status = Command::new("open")
            .args(["-n"])
            .arg(&app_path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .context("failed to `open` the app")?;
        if !status.success() {
            bail!("`open` returned non-zero — is the app bundle valid?");
        }

        // Give the app time to boot the webview + first paint().
        std::thread::sleep(Duration::from_secs(4));

        // Activate the app: WKWebView's AX tree only fully exposes itself once
        // the window is brought to the foreground. Without this, AXWebArea may
        // read as absent right after launch.
        let _ = Command::new("osascript")
            .args(["-e", "tell application \"Agent Deck\" to activate"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        std::thread::sleep(Duration::from_secs(2));

        let mut ax = AxClient::spawn()?;

        // Health check + wait for the webview to be reachable.
        let resp = ax.call("ping", json!({}))?;
        if resp.get("ok") != Some(&Value::Bool(true)) {
            bail!("swift driver ping failed: {resp}");
        }

        Ok(Self {
            ax,
            _fixture: fixture,
        })
    }

    /// Wait until a CSS-like selector resolves in the webview.
    pub fn wait(&mut self, selector: &str) -> Result<()> {
        let resp = self
            .ax
            .call("wait", json!({ "selector": selector, "ms": 8000 }))?;
        if resp.get("ok") == Some(&Value::Bool(true)) {
            Ok(())
        } else {
            bail!("timeout waiting for '{selector}': {resp}")
        }
    }

    /// Count elements matching a selector.
    pub fn count(&mut self, selector: &str) -> Result<usize> {
        let resp = self.ax.call("count", json!({ "selector": selector }))?;
        resp.get("count")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize)
            .ok_or_else(|| anyhow!("bad count response: {resp}"))
    }

    /// Click (press) the first element matching a selector.
    pub fn click(&mut self, selector: &str) -> Result<()> {
        let resp = self
            .ax
            .call("click", json!({ "selector": selector, "ms": 8000 }))?;
        if resp.get("ok") == Some(&Value::Bool(true)) {
            Ok(())
        } else {
            bail!("click '{selector}' failed: {resp}")
        }
    }

    /// Read the AXValue (and AXTitle) of the first element matching a selector.
    pub fn value(&mut self, selector: &str) -> Result<(String, String)> {
        let resp = self
            .ax
            .call("get_value", json!({ "selector": selector, "ms": 8000 }))?;
        let v = resp
            .get("value")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let t = resp
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        Ok((v, t))
    }

    /// Is the app's webview still reachable?
    pub fn alive(&mut self) -> Result<bool> {
        let resp = self.ax.call("alive", json!({}))?;
        Ok(resp.get("running") == Some(&Value::Bool(true)))
    }
}

impl Drop for E2e {
    fn drop(&mut self) {
        // `ax` owns the swift driver child; dropping it closes stdin and reaps
        // the process. Then quit the launched app so it doesn't linger.
        let _ = Command::new("osascript")
            .args(["-e", "tell application \"Agent Deck\" to quit"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}
