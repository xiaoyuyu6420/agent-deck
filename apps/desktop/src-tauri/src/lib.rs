use agent_deck_host_core::{DesktopService, HostConfig, SessionInfo};
use agent_deck_protocol::{home_dir, BackendId, BoardState, LedFrame};
use serde::Serialize;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager, State, WindowEvent,
};

struct AppState {
    service: Mutex<DesktopService>,
}

#[derive(Clone, Serialize)]
struct BoardUpdate {
    board: BoardState,
    leds: LedFrame,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SettingsView {
    auto_fill: bool,
    done_ttl_after_open_ms: u64,
    done_ttl_unopened_ms: u64,
}

fn default_config() -> HostConfig {
    let home = home_dir();
    // Do NOT exclude the current repo — user may want to bind its sessions.
    HostConfig {
        tasks_db_path: home.join(".zcode/v2/tasks-index.sqlite"),
        tool_db_path: home.join(".zcode/cli/db/db.sqlite"),
        exclude_workspaces: vec![],
        exclude_task_ids: vec![],
        slot_count: 8,
        enable_codex: true,
        codex_cli_path: None,
        enable_workbuddy: true,
        workbuddy_projects_dir: None,
    }
}

#[tauri::command]
fn get_board_state(state: State<'_, Arc<AppState>>) -> Result<BoardState, String> {
    let service = state.service.lock().map_err(|e| e.to_string())?;
    Ok(service.board_state())
}

#[tauri::command]
fn get_led_frame(state: State<'_, Arc<AppState>>) -> Result<LedFrame, String> {
    let service = state.service.lock().map_err(|e| e.to_string())?;
    Ok(service.led_frame())
}

#[tauri::command]
fn set_focus(state: State<'_, Arc<AppState>>, i: usize) -> Result<(), String> {
    let mut service = state.service.lock().map_err(|e| e.to_string())?;
    service.set_focus(i);
    Ok(())
}

/// Click a bound key: focus the slot and open the corresponding backend session.
#[tauri::command]
fn open_slot_session(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    i: usize,
) -> Result<(), String> {
    let mut service = state.service.lock().map_err(|e| e.to_string())?;
    service.set_focus(i);
    let board = service.board_state();
    let slot = board
        .slots
        .iter()
        .find(|s| s.i == i)
        .cloned()
        .ok_or_else(|| format!("slot {i} not found"))?;

    let Some(session_id) = slot.session_id.clone() else {
        return Err("empty slot — long-press to bind a session".into());
    };
    let backend = slot.backend.unwrap_or(BackendId::Zcode);
    // Mark opened immediately on key click (before backend app launch). This
    // starts WorkBuddy's short Done TTL; success of the external open is
    // intentionally irrelevant.
    service.mark_opened(backend, &session_id);
    // Prefer live catalog for workspace path (board state doesn't carry it).
    let catalog = service.list_sessions();
    let info = catalog.iter().find(|s| s.session_id == session_id).cloned();
    drop(service);

    match backend {
        BackendId::Zcode => open_zcode_session(&app, &session_id, info.as_ref())?,
        BackendId::Codex => open_codex_session(&app, &session_id, info.as_ref())?,
        BackendId::Workbuddy => open_workbuddy_session(&app, &session_id, info.as_ref())?,
    }
    Ok(())
}

fn open_zcode_session(
    _app: &AppHandle,
    _session_id: &str,
    info: Option<&SessionInfo>,
) -> Result<(), String> {
    // How to switch a RUNNING ZCode (3.4.2) to a workspace. Each option was
    // empirically verified on this machine (2026-07-23), measuring the main
    // process PID before/after to judge safety:
    //
    //   (A) `open -a ZCode --args --open-workspace <p>`  →  NO effect.
    //       LaunchServices sees the app is running and just activates it,
    //       dropping --args entirely. Safe (no new process) but useless.
    //
    //   (B) spawn the Mach-O binary / `open -n -a ZCode --args ...`  →
    //       DANGEROUS. Both create a second ZCode process that calls
    //       requestSingleInstanceLock. Twice this crashed the user's LIVE
    //       ZCode session (the running instance quit and a fresh one took
    //       its place) — contrary to Electron's "loser quits, holder is safe"
    //       theory. NEVER use any path that starts a second ZCode process.
    //
    //   (C) `open "zcode://workspace/open?path=<urlencoded p>"`  →  WORKS
    //       and SAFE. URL dispatch is delivered to the already-running
    //       instance's handleOpenURL — no second process, no lock fight,
    //       PID stays constant (verified twice). It DOES show
    //       confirmExternalWorkspaceOpen ("Open this folder in ZCode?") every
    //       time — that handler (Fk) is unconditional, has no allowlist and no
    //       "don't ask again" persistence. The user confirms once per click.
    //
    //   Reverse-engineering dead ends (all verified, all closed): no
    //   trusted-workspace allowlist (allowedWorkspaces is an unrelated bot
    //   config), no desktop plugin loading, web-remote-control is cloud-relay
    //   only with no "open local path" command, and app.asar is sealed by
    //   SHA256 integrity + Hardened Runtime signature so it can't be patched.
    //   So (C) is the only safe way that actually switches the project.
    //
    // session_id is accepted but unused: ZCode 3.4.2 exposes no external
    // entry point to a specific task/session (setActiveTaskId is in-process
    // IPC only; ACP session/resume hydrates an unrelated headless copy from
    // disk and cannot drive the desktop window). We land on the right project
    // and the user picks the session there.
    let workspace = info.and_then(|s| s.workspace_path.as_deref());
    if let Some(path) = workspace {
        if !path.is_empty() && path != "(unknown project)" {
            let encoded = url_encode_path(path);
            let url = format!("zcode://workspace/open?path={encoded}");
            open_url(&url).map_err(|e| format!("open ZCode workspace failed: {e}"))?;
            return Ok(());
        }
    }
    // No workspace known — just activate/focus the existing ZCode window.
    launch_app("ZCode").map_err(|e| format!("open ZCode failed: {e}"))?;
    Ok(())
}

/// Percent-encode a filesystem path for use in a `zcode://` query value.
/// Encodes everything except the unreserved set [A-Za-z0-9-._~] and '/' (kept
/// literal so paths stay readable); spaces and non-ASCII (e.g. Chinese) become
/// %XX / %uXXXX-style UTF-8 percent-encoding. Matches what ZCode's
/// extractWorkspaceOpenPath expects (it reads URL.searchParams.get("path")).
fn url_encode_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for &b in path.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

fn open_codex_session(
    _app: &AppHandle,
    session_id: &str,
    _info: Option<&SessionInfo>,
) -> Result<(), String> {
    // Verified 2026-07-23 against ChatGPT.app (codex-cli 0.145.0-alpha.27):
    //   - ChatGPT.app registers the `codex://` URL scheme (LaunchServices:
    //     bundle "ChatGPT" claims scheme "codex:").
    //   - The renderer builds `codex://threads/<threadId>` for the "Open in app"
    //     menu item and the "copyAppLink" action (5 call sites across the
    //     unpacked app.asar). <threadId> = `thread.id` (the rollout UUID), NOT
    //     the rollout's `session_id` (which is a git/worktree session id, a
    //     distinct concept — see crates/codex/src/mapper.rs).
    //   - No second process is spawned, so no single-instance lock fight
    //     (unlike spawning the codex binary directly — see the analogous ZCode
    //     (B) dead-end in open_zcode_session).
    //   - The app-server protocol independently supports `thread/resume
    //     {threadId}` to rejoin a thread (rejoin, not a headless copy — unlike
    //     ZCode's ACP session/resume). That RPC path is not needed here because
    //     the deep link already drives the desktop window; it is documented in
    //     docs/codex-integration.md for future use (e.g. in-app control).
    //
    // `session_id` here is `thread.id` (see crates/codex/src/mapper.rs).
    //
    // Cold-start caveat (empirically verified 2026-07-23): when ChatGPT.app is
    // NOT already running, dispatching the deep link launches the app and lands
    // on the right *project*, but the URL is swallowed during early startup and
    // the specific thread is NOT navigated to (the new-thread landing page is
    // shown instead). When the app IS already running, the same deep link jumps
    // to the exact thread reliably and repeatably (verified across two distinct
    // thread ids). Fix: on cold start, bring the app up first and wait for its
    // main process to be ready, THEN dispatch the deep link.
    if !app_running("ChatGPT.app/Contents/MacOS/ChatGPT") {
        // Launch without a URL so the app reaches its ready state, then wait.
        // On Windows the pattern is the exe name.
        #[cfg(target_os = "macos")]
        let _ = launch_app("ChatGPT");
        #[cfg(not(target_os = "macos"))]
        let _ = launch_app("ChatGPT");
        wait_for_chatgpt_ready();
    }
    let url = format!("codex://threads/{session_id}");
    open_url(&url).map_err(|e| format!("open codex thread failed: {e}"))?;
    Ok(())
}

/// Whether the ChatGPT.app GUI main process is running. Distinguishes the GUI
/// from the bundled `codex app-server` child (which has a different exec path
/// under Contents/Resources).
fn chatgpt_app_running() -> bool {
    // macOS matches the .app bundle exec path; Windows matches the exe name.
    let pattern = if cfg!(target_os = "macos") {
        "ChatGPT.app/Contents/MacOS/ChatGPT"
    } else {
        "ChatGPT.exe"
    };
    app_running(pattern)
}

/// Poll for the ChatGPT.app main process to appear (cold-start readiness).
/// Bounded so a click never blocks the UI thread for long.
fn wait_for_chatgpt_ready() {
    for _ in 0..40 {
        if chatgpt_app_running() {
            // Process exists; give the renderer a beat to register its URL
            // handler before we dispatch.
            std::thread::sleep(Duration::from_millis(500));
            return;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

fn open_workbuddy_session(
    _app: &AppHandle,
    session_id: &str,
    _info: Option<&SessionInfo>,
) -> Result<(), String> {
    // Verified 2026-07-23 against WorkBuddy.app (v5.2.6, bundle com.workbuddy.workbuddy):
    //   - Info.plist CFBundleURLSchemes registers `workbuddy`.
    //   - Renderer maps route `/task/<sessionId>` ↔ deeplink `workbuddy://chat/<sessionId>`
    //     (ROUTE_PREFIX_TO_DEEPLINK_HOST: ["/task","chat"]).
    //   - Main process has early-open-url capture + renderer queue so cold-start
    //     deep links are buffered rather than swallowed (unlike early ChatGPT).
    //   - session_id is the jsonl basename / `--session-id` / event.sessionId
    //     (see crates/workbuddy + docs/workbuddy-integration.md).
    //
    // Still do a light cold-start warm-up when the GUI isn't running: launch the
    // app first, wait for the main Electron process, then dispatch the URL so
    // the first paint has a better chance of landing on /task/<id>.
    if !workbuddy_app_running() {
        let _ = launch_app("WorkBuddy");
        wait_for_workbuddy_ready();
    }
    let url = format!("workbuddy://chat/{session_id}");
    open_url(&url).map_err(|e| format!("open WorkBuddy task failed: {e}"))?;
    Ok(())
}

/// Whether the WorkBuddy.app GUI main process is running.
///
/// Matches `WorkBuddy.app/Contents/MacOS/Electron` (the app binary name is
/// Electron). Child daemons also use that path with extra args, but presence of
/// any WorkBuddy.app process is enough for "app is up" warm-up purposes.
fn workbuddy_app_running() -> bool {
    let pattern = if cfg!(target_os = "macos") {
        "WorkBuddy.app/Contents/MacOS"
    } else {
        "WorkBuddy.exe"
    };
    app_running(pattern)
}

fn wait_for_workbuddy_ready() {
    for _ in 0..40 {
        if workbuddy_app_running() {
            std::thread::sleep(Duration::from_millis(500));
            return;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

// ─── 平台分派：打开 URL / 启动 App / 检测进程 ─────────────────────────────
//
// macOS 用 `open(1)`；Linux 用 `xdg-open`；Windows 用 `cmd /C start`。
// 进程检测：macOS/Linux 用 `pgrep -f`，Windows 用 `tasklist`。

/// Dispatch a URL (incl. custom scheme like `codex://`, `workbuddy://`,
/// `zcode://`) to the OS handler. Non-fatal: returns Ok even if the handler
/// isn't registered (the app simply won't come up).
fn open_url(url: &str) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        Command::new("open").arg(url).status().map(|_| ())
    }
    #[cfg(target_os = "windows")]
    {
        // `start "" <url>` — the empty title arg is required so the URL itself
        // isn't mistaken for a window title.
        Command::new("cmd")
            .args(["/C", "start", "", url])
            .status()
            .map(|_| ())
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        Command::new("xdg-open").arg(url).status().map(|_| ())
    }
}

/// Launch / focus an installed app by name (no URL). macOS uses
/// `open -a <app>`; Windows launches by exe name; Linux best-effort.
fn launch_app(app: &str) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        Command::new("open").args(["-a", app]).status().map(|_| ())
    }
    #[cfg(target_os = "windows")]
    {
        // Best-effort: app.exe on PATH or registered App Paths.
        Command::new("cmd")
            .args(["/C", "start", "", &format!("{app}.exe")])
            .status()
            .map(|_| ())
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let _ = app;
        Ok(())
    }
}

/// Whether *any* process matching `pattern` is running.
///
/// - macOS/Linux: `pgrep -f <pattern>` (pattern is a substring of the cmdline).
/// - Windows: `tasklist` filtered by image name; `pattern` should be an exe
///   name like `"ChatGPT.exe"`. We check stdout for the image name.
fn app_running(pattern: &str) -> bool {
    #[cfg(any(target_os = "macos", all(unix, not(target_os = "macos"))))]
    {
        Command::new("pgrep")
            .args(["-f", pattern])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
    #[cfg(target_os = "windows")]
    {
        let output = Command::new("tasklist")
            .args(["/FI", &format!("IMAGENAME eq {pattern}"), "/FO", "CSV", "/NH"])
            .output();
        match output {
            Ok(o) => {
                let stdout = String::from_utf8_lossy(&o.stdout);
                stdout.contains(pattern)
            }
            Err(_) => false,
        }
    }
}

/// Open an arbitrary URL/path via system `open(1)`.
///
/// Currently unused (the codex path switched to a `codex://threads/...` deep
/// link, and zcode builds its own URL inline) but kept as the canonical helper
/// for any future external-open fallback (e.g. a web URL when a scheme isn't
/// registered).
#[allow(dead_code)]
fn open_external(_app: &AppHandle, url: &str) -> Result<(), String> {
    // Delegate to the platform-aware helper instead of raw `open(1)`.
    open_url(url).map_err(|e| format!("open failed: {e}"))?;
    Ok(())
}

#[tauri::command]
fn pin_slot(
    state: State<'_, Arc<AppState>>,
    i: usize,
    session_id: Option<String>,
) -> Result<(), String> {
    let mut service = state.service.lock().map_err(|e| e.to_string())?;
    service.pin_slot(i, session_id);
    Ok(())
}

#[tauri::command]
fn list_sessions(state: State<'_, Arc<AppState>>) -> Result<Vec<SessionInfo>, String> {
    let mut service = state.service.lock().map_err(|e| e.to_string())?;
    // Catalog query (not board poll): all projects + historical sessions.
    Ok(service.list_sessions())
}

#[tauri::command]
fn get_settings(state: State<'_, Arc<AppState>>) -> Result<SettingsView, String> {
    let service = state.service.lock().map_err(|e| e.to_string())?;
    let s = service.settings();
    Ok(SettingsView {
        auto_fill: s.auto_fill,
        done_ttl_after_open_ms: s.done_ttl_after_open_ms,
        done_ttl_unopened_ms: s.done_ttl_unopened_ms,
    })
}

#[tauri::command]
fn set_auto_fill(state: State<'_, Arc<AppState>>, enabled: bool) -> Result<(), String> {
    let mut service = state.service.lock().map_err(|e| e.to_string())?;
    service.set_auto_fill(enabled);
    Ok(())
}

#[tauri::command]
fn set_done_ttl(
    state: State<'_, Arc<AppState>>,
    after_open_ms: u64,
    unopened_ms: u64,
) -> Result<(), String> {
    let mut service = state.service.lock().map_err(|e| e.to_string())?;
    service.set_done_ttl(after_open_ms, unopened_ms);
    Ok(())
}

#[tauri::command]
fn hide_window(app: AppHandle) -> Result<(), String> {
    hide_main_window(&app);
    Ok(())
}

#[tauri::command]
fn minimize_window(app: AppHandle) -> Result<(), String> {
    minimize_main_window(&app);
    Ok(())
}

#[tauri::command]
fn show_window(app: AppHandle) -> Result<(), String> {
    show_main_window(&app);
    Ok(())
}

#[tauri::command]
fn start_dragging(app: AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window("main") {
        window.start_dragging().map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
fn dispatch_action(state: State<'_, Arc<AppState>>, action: String) -> Result<String, String> {
    let mut service = state.service.lock().map_err(|e| e.to_string())?;
    Ok(service.dispatch_action(&action))
}

fn show_main_window(app: &AppHandle) {
    // macOS: a transparent, borderless, always-on-top panel that was hidden
    // via window.hide() does NOT come back from a Dock click on its own —
    // Tauri only surfaces RunEvent::Reopen, it won't re-show the window. The
    // Reopen handler in run() calls us. We must explicitly activate the app
    // (Regular policy guarantees the Dock can raise it) and then show+focus.
    #[cfg(target_os = "macos")]
    {
        let _ = app.set_activation_policy(tauri::ActivationPolicy::Regular);
    }
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
        #[cfg(target_os = "macos")]
        {
            // Toggle always-on-top off/on to force the panel back to the top
            // of the window layer even while other apps are focused.
            let _ = window.set_always_on_top(false);
            let _ = window.set_always_on_top(true);
        }
    }
}

fn hide_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.hide();
    }
}

/// Minimize the panel to the Dock. Unlike `hide()`, a minimized window is
/// restored by clicking the Dock icon (native macOS behavior), so the user is
/// never left with no way to bring it back.
fn minimize_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.minimize();
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let service = DesktopService::new(default_config()).expect("init desktop service");
    let state = Arc::new(AppState {
        service: Mutex::new(service),
    });

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .manage(state.clone())
        .invoke_handler(tauri::generate_handler![
            get_board_state,
            get_led_frame,
            set_focus,
            open_slot_session,
            pin_slot,
            list_sessions,
            get_settings,
            set_auto_fill,
            set_done_ttl,
            hide_window,
            minimize_window,
            show_window,
            start_dragging,
            dispatch_action
        ])
        .setup(move |app| {
            // macOS: pin the activation policy to Regular at startup so the
            // app is a first-class Dock citizen. This is what makes clicking
            // the Dock icon fire RunEvent::Reopen (handled in run()), which is
            // the only reliable way to surface a window that was hidden via
            // window.hide(). Without this, a borderless transparent panel can
            // end up in a state where Dock clicks do nothing.
            #[cfg(target_os = "macos")]
            {
                let _ = app.set_activation_policy(tauri::ActivationPolicy::Regular);
            }

            let show_i = MenuItem::with_id(app, "show", "显示悬浮窗", true, None::<&str>)?;
            let hide_i = MenuItem::with_id(app, "hide", "隐藏悬浮窗", true, None::<&str>)?;
            let quit_i = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show_i, &hide_i, &quit_i])?;

            // Keep tray icon alive for the whole app lifetime.
            // macOS: left-click must NOT open the menu, otherwise Click never
            // fires and the window appears "gone forever" after hide.
            // The icon MUST be set explicitly — without it Tauri v2 shows no
            // status-bar item at all, leaving the user with no way to unhide.
            let mut builder = TrayIconBuilder::with_id("main-tray")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .tooltip("Agent Deck — 左键显示/隐藏，右键菜单");
            if let Some(icon) = app.default_window_icon() {
                builder = builder.icon(icon.clone());
            }
            let tray = builder
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => show_main_window(app),
                    "hide" => hide_main_window(app),
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        // Toggle show/hide on left click.
                        let app = tray.app_handle();
                        if let Some(window) = app.get_webview_window("main") {
                            if window.is_visible().unwrap_or(false) {
                                hide_main_window(app);
                            } else {
                                show_main_window(app);
                            }
                        }
                    }
                })
                .build(app)?;
            app.manage(tray);

            if let Some(window) = app.get_webview_window("main") {
                let app_handle = app.handle().clone();
                window.on_window_event(move |event| {
                    if let WindowEvent::CloseRequested { api, .. } = event {
                        api.prevent_close();
                        hide_main_window(&app_handle);
                    }
                });
            }

            let app_handle = app.handle().clone();
            let poll_state = state.clone();
            thread::spawn(move || loop {
                // 200ms poll. State comes from tool_usage (written in real time
                // by ZCode as each tool starts/finishes), and a bound key must
                // reflect a tool kicking off within a beat — 500ms felt laggy
                // ("several seconds" behind). 200ms is still cheap for local
                // read-only sqlite (sub-ms per query) and reads as instant.
                thread::sleep(Duration::from_millis(200));
                let update = {
                    let mut service = match poll_state.service.lock() {
                        Ok(s) => s,
                        Err(_) => continue,
                    };
                    if service.tick().is_err() {
                        continue;
                    }
                    BoardUpdate {
                        board: service.board_state(),
                        leds: service.led_frame(),
                    }
                };
                let _ = app_handle.emit("board-updated", update);
            });

            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building Agent Deck")
        .run(|app_handle, event| {
            // macOS: clicking the Dock icon when the window is hidden/minimized
            // fires RunEvent::Reopen. Bring the panel back so the user always
            // has a way to surface the window.
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen { .. } = event {
                show_main_window(app_handle);
            }
        });
}

#[cfg(test)]
mod tests {
    use super::url_encode_path;

    #[test]
    fn url_encode_keeps_unreserved_and_slash() {
        // ASCII unreserved set + '/' stay literal.
        assert_eq!(
            url_encode_path("/Users/munich/code/proxy-pool_v2.0"),
            "/Users/munich/code/proxy-pool_v2.0"
        );
    }

    #[test]
    fn url_encode_encodes_space() {
        assert_eq!(url_encode_path("/a/b c"), "/a/b%20c");
    }

    #[test]
    fn url_encode_encodes_chinese_path() {
        // Chinese in the path (common here: 独立项目) must become UTF-8
        // percent-encoding, exactly what URLSearchParams on the ZCode side
        // will decode back to the original string.
        let encoded = url_encode_path("/Users/munich/Desktop/独立项目/modjing");
        assert!(encoded.starts_with("/Users/munich/Desktop/"));
        assert!(!encoded.contains('独'));
        // 独立项目 in UTF-8 bytes, percent-encoded:
        assert_eq!(
            encoded,
            "/Users/munich/Desktop/%E7%8B%AC%E7%AB%8B%E9%A1%B9%E7%9B%AE/modjing"
        );
    }
}
