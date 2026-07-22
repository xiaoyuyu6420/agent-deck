use agent_deck_host_core::{DesktopService, HostConfig};
use agent_deck_protocol::{BoardState, LedFrame};
use serde::Serialize;
use std::path::PathBuf;
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

fn default_config() -> HostConfig {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    HostConfig {
        tasks_db_path: home.join(".zcode/v2/tasks-index.sqlite"),
        tool_db_path: home.join(".zcode/cli/db/db.sqlite"),
        exclude_workspaces: vec![
            cwd.to_string_lossy().to_string(),
            home.join("Desktop/独立项目/codex 键盘")
                .to_string_lossy()
                .to_string(),
        ],
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
fn dispatch_action(state: State<'_, Arc<AppState>>, action: String) -> Result<String, String> {
    let service = state.service.lock().map_err(|e| e.to_string())?;
    Ok(service.dispatch_action(&action))
}

fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.set_focus();
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
            pin_slot,
            dispatch_action
        ])
        .setup(move |app| {
            let show_i = MenuItem::with_id(app, "show", "显示悬浮窗", true, None::<&str>)?;
            let hide_i = MenuItem::with_id(app, "hide", "隐藏悬浮窗", true, None::<&str>)?;
            let quit_i = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show_i, &hide_i, &quit_i])?;

            let _tray = TrayIconBuilder::new()
                .menu(&menu)
                .tooltip("Agent Deck")
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
                        show_main_window(tray.app_handle());
                    }
                })
                .build(app)?;

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
