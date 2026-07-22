use agent_deck_host_core::{DesktopService, HostConfig, SessionInfo};
use agent_deck_protocol::{BackendId, BoardState, LedFrame};
use serde::Serialize;
use std::path::PathBuf;
use std::process::Command;
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
}

fn default_config() -> HostConfig {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    // Do NOT exclude the current repo — user may want to bind its sessions.
    HostConfig {
        tasks_db_path: home.join(".zcode/v2/tasks-index.sqlite"),
        tool_db_path: home.join(".zcode/cli/db/db.sqlite"),
        exclude_workspaces: vec![],
        exclude_task_ids: vec![],
        slot_count: 8,
        enable_codex: true,
        codex_cli_path: None,
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
    drop(service);

    let Some(session_id) = slot.session_id.clone() else {
        return Err("empty slot — long-press to bind a session".into());
    };
    let backend = slot.backend.unwrap_or(BackendId::Zcode);
    // Prefer live catalog for workspace path (board state doesn't carry it).
    let mut service = state.service.lock().map_err(|e| e.to_string())?;
    let catalog = service.list_sessions();
    let info = catalog.iter().find(|s| s.session_id == session_id).cloned();
    drop(service);

    match backend {
        BackendId::Zcode => open_zcode_session(&app, &session_id, info.as_ref())?,
        BackendId::Codex => open_codex_session(&app, &session_id, info.as_ref())?,
    }
    Ok(())
}

fn open_zcode_session(
    _app: &AppHandle,
    _session_id: &str,
    info: Option<&SessionInfo>,
) -> Result<(), String> {
    // There are two ways to ask a running ZCode to open a workspace:
    //
    //   (A) deep-link URL  `zcode://workspace/open?path=<p>`
    //       → handleDeepLink (Rd) → ALWAYS calls confirmExternalWorkspaceOpen
    //       (bk) → the scary "Only open folders from sources you trust" dialog.
    //
    //   (B) spawn the binary with  `ZCode --open-workspace <p>`
    //       → Electron requestSingleInstanceLock routes this to the running
    //       instance as a second-instance event → handleSecondInstanceWorkspace
    //       Request (TT) → wo(argv) → handleOpenWorkspacePath (Ck) →
    //       webContents.send(OpenWorkspacePath). Ck NEVER calls bk → no trust
    //       dialog, and it switches the existing window to that project's tab.
    //
    //   IMPORTANT: `open -a ZCode --args --open-workspace <p>` does NOT work —
    //   LaunchServices sees the app is running and just activates it, dropping
    //   --args entirely (log shows "reused existing window (app-activate)" with
    //   no second-instance). Must spawn the Mach-O binary directly so Electron
    //   sees a second process and fires its second-instance handler.
    //
    // We use (B). task-level (setActiveTaskId) has no external entry point in
    // ZCode 3.4.2 (TaskNotificationClick is in-process IPC only), so we land
    // on the correct project tab and let the user pick the session there.
    let workspace = info.and_then(|s| s.workspace_path.as_deref());
    if let Some(path) = workspace {
        if !path.is_empty() && path != "(unknown project)" {
            let bin = "/Applications/ZCode.app/Contents/MacOS/ZCode";
            if PathBuf::from(bin).exists() {
                Command::new(bin)
                    .args(["--open-workspace", path])
                    .spawn()
                    .map_err(|e| format!("spawn ZCode failed: {e}"))?;
                return Ok(());
            }
            // ZCode not in /Applications — fall back to open -a (may show trust
            // dialog via deep-link, but better than nothing).
            Command::new("open")
                .args(["-a", "ZCode", "--args", "--open-workspace", path])
                .status()
                .map_err(|e| format!("open ZCode workspace failed: {e}"))?;
            return Ok(());
        }
    }
    // No workspace known — just launch/focus ZCode.
    Command::new("open")
        .arg("-a")
        .arg("ZCode")
        .status()
        .map_err(|e| format!("open ZCode failed: {e}"))?;
    Ok(())
}

fn open_codex_session(
    app: &AppHandle,
    session_id: &str,
    info: Option<&SessionInfo>,
) -> Result<(), String> {
    // Codex has no stable public session URL on this machine; open CLI resume
    // in Terminal when possible, otherwise just open the workspace folder.
    if let Some(path) = info.and_then(|s| s.workspace_path.as_deref()) {
        if !path.is_empty() {
            let _ = Command::new("open").arg(path).status();
        }
    }
    // Prefer `codex resume <id>` in Terminal via osascript (macOS).
    let script = format!(
        r#"tell application "Terminal"
  activate
  do script "codex resume {session_id}"
end tell"#
    );
    let status = Command::new("osascript")
        .arg("-e")
        .arg(script)
        .status()
        .map_err(|e| format!("osascript failed: {e}"))?;
    if !status.success() {
        // Fallback: just open codex app/cli if present.
        let _ = open_external(app, "https://chatgpt.com/codex");
    }
    Ok(())
}

fn open_external(_app: &AppHandle, url: &str) -> Result<(), String> {
    // Use system open(1). Avoid tauri-plugin-shell::open (deprecated → opener).
    Command::new("open")
        .arg(url)
        .status()
        .map_err(|e| format!("open(1) failed: {e}"))?;
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
    Ok(SettingsView {
        auto_fill: service.settings().auto_fill,
    })
}

#[tauri::command]
fn set_auto_fill(state: State<'_, Arc<AppState>>, enabled: bool) -> Result<(), String> {
    let mut service = state.service.lock().map_err(|e| e.to_string())?;
    service.set_auto_fill(enabled);
    Ok(())
}

#[tauri::command]
fn hide_window(app: AppHandle) -> Result<(), String> {
    hide_main_window(&app);
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
    let service = state.service.lock().map_err(|e| e.to_string())?;
    Ok(service.dispatch_action(&action))
}

fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
        // macOS: dock-less always-on-top panel can need an extra focus nudge.
        #[cfg(target_os = "macos")]
        {
            let _ = window.set_always_on_top(true);
        }
    }
}

fn hide_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.hide();
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
            hide_window,
            show_window,
            start_dragging,
            dispatch_action
        ])
        .setup(move |app| {
            let show_i = MenuItem::with_id(app, "show", "显示悬浮窗", true, None::<&str>)?;
            let hide_i = MenuItem::with_id(app, "hide", "隐藏悬浮窗", true, None::<&str>)?;
            let quit_i = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show_i, &hide_i, &quit_i])?;

            // Keep tray icon alive for the whole app lifetime.
            // macOS: left-click must NOT open the menu, otherwise Click never
            // fires and the window appears "gone forever" after hide.
            let tray = TrayIconBuilder::new()
                .menu(&menu)
                .show_menu_on_left_click(false)
                .tooltip("Agent Deck — 左键显示/隐藏，右键菜单")
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
                thread::sleep(Duration::from_millis(500));
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
        .run(tauri::generate_context!())
        .expect("error while running Agent Deck");
}
