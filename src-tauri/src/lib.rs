//! DSh Desktop — Tauri application shell.
//!
//! An original desktop client for DeepSeek Harness: manages the official
//! `dsh web` server lifecycle (spawn/attach/watchdog/logs), provides a
//! branded console page, native clipboard bridging for the web UI, and the
//! standard macOS Edit menu (keyboard copy/paste).

mod menu;
mod server;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

use server::{DshServer, ServerState};
use tauri::{AppHandle, Manager, WindowEvent};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};
use tauri_plugin_opener::OpenerExt;

pub struct AppState {
    pub server: DshServer,
    /// URL of the local console page, captured at setup so we can navigate
    /// back to it whenever the service stops.
    pub console_url: Mutex<Option<String>>,
    /// Whether the one-time "minimized to tray" notice has fired this run.
    hide_notice_shown: AtomicBool,
}

impl Clone for AppState {
    fn clone(&self) -> Self {
        Self {
            server: self.server.clone(),
            console_url: Mutex::new(self.console_url.lock().unwrap().clone()),
            hide_notice_shown: AtomicBool::new(self.hide_notice_shown.load(Ordering::Relaxed)),
        }
    }
}

// ── commands (console page) ─────────────────────────────────────────────────

#[tauri::command]
fn status(state: tauri::State<'_, AppState>) -> server::ServerStatus {
    state.server.status()
}

#[tauri::command]
fn logs(state: tauri::State<'_, AppState>, n: Option<usize>) -> Vec<server::LogLine> {
    state.server.logs(n.unwrap_or(200))
}

#[tauri::command]
fn start(state: tauri::State<'_, AppState>) -> Result<(), String> {
    let srv = state.server.clone();
    thread::spawn(move || srv.start());
    Ok(())
}

#[tauri::command]
fn restart(state: tauri::State<'_, AppState>) -> Result<(), String> {
    let srv = state.server.clone();
    thread::spawn(move || {
        srv.stop();
        thread::sleep(Duration::from_millis(600));
        let _ = srv.start();
    });
    Ok(())
}

#[tauri::command]
fn stop(state: tauri::State<'_, AppState>) -> Result<(), String> {
    state.server.stop();
    Ok(())
}

#[derive(serde::Serialize)]
struct Info {
    version: String,
    dsh_version: Option<String>,
    data_dir: String,
    port: u16,
}

#[tauri::command]
fn info(_app: AppHandle, state: tauri::State<'_, AppState>) -> Info {
    let dsh_version = dsh_version(&state.server);
    let data_dir = std::env::home_dir().map(|h| h.join(".dsh")).unwrap_or_default();
    Info {
        version: env!("CARGO_PKG_VERSION").to_string(),
        dsh_version,
        data_dir: data_dir.display().to_string(),
        port: DshServer::port_override().unwrap_or(server::DEFAULT_PORT),
    }
}

#[tauri::command]
fn open_browser(app: AppHandle) -> Result<(), String> {
    let port = DshServer::port_override().unwrap_or(server::DEFAULT_PORT);
    let url = format!("http://127.0.0.1:{port}");
    app.opener().open_url(url, None::<&str>).map_err(|e| e.to_string())
}

#[tauri::command]
fn open_data_dir(app: AppHandle) -> Result<(), String> {
    let dir = std::env::home_dir().map(|h| h.join(".dsh")).unwrap_or_default();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    app.opener().reveal_item_in_dir(dir).map_err(|e| e.to_string())
}

#[tauri::command]
fn autostart_enabled(app: AppHandle) -> bool {
    app.autolaunch().is_enabled().unwrap_or(false)
}

#[tauri::command]
fn autostart_set(app: AppHandle, enabled: bool) -> Result<(), String> {
    if enabled {
        app.autolaunch().enable().map_err(|e| e.to_string())
    } else {
        app.autolaunch().disable().map_err(|e| e.to_string())
    }
}

// ── helpers ─────────────────────────────────────────────────────────────────

/// Read the installed dsh package version from the bundled runtime.
fn dsh_version(server: &DshServer) -> Option<String> {
    let pkg = server.runtime_paths().into_iter().find_map(|base| {
        let p = base.join("node_modules").join("@deepseek-ai").join("dsh").join("package.json");
        p.is_file().then_some(p)
    })?;
    let text = std::fs::read_to_string(pkg).ok()?;
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    json.get("version")?.as_str().map(|s| s.to_string())
}

fn show_main_window(app: &AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.show();
        let _ = win.unminimize();
        let _ = win.set_focus();
    }
}

fn handle_menu_action(app: &AppHandle, id: &str) {
    let state = app.state::<AppState>();
    match id {
        menu::MENU_OPEN_BROWSER => {
            let _ = open_browser(app.clone());
        }
        menu::MENU_OPEN_DATA_DIR => {
            let _ = open_data_dir(app.clone());
        }
        menu::MENU_RESTART => {
            let srv = state.server.clone();
            thread::spawn(move || {
                srv.stop();
                thread::sleep(Duration::from_millis(600));
                let _ = srv.start();
            });
        }
        menu::MENU_CONSOLE => {
            if let Some(url) = state.console_url.lock().unwrap().clone() {
                if let Some(win) = app.get_webview_window("main") {
                    let _ = win.navigate(url.parse().expect("console url"));
                }
            }
        }
        menu::MENU_RELOAD => {
            if let Some(win) = app.get_webview_window("main") {
                let _ = win.eval("window.location.reload()");
            }
        }
        menu::MENU_SHOW_WINDOW => show_main_window(app),
        menu::MENU_QUIT => {
            state.server.stop();
            app.exit(0);
        }
        _ => {}
    }
}

/// Watch the server state and move the webview between the console page and
/// the dsh web UI accordingly.
fn spawn_navigation_watcher(app: AppHandle, state: AppState) {
    thread::spawn(move || {
        let mut last = ServerState::Stopped;
        loop {
            let current = state.server.status();
            let cur_state = match current.state.as_str() {
                "running" => ServerState::Running,
                "starting" => ServerState::Starting,
                "error" => ServerState::Error,
                _ => ServerState::Stopped,
            };
            if cur_state != last {
                let port = current.port.unwrap_or_else(|| DshServer::port_override().unwrap_or(server::DEFAULT_PORT));
                if cur_state == ServerState::Running {
                    if let Some(win) = app.get_webview_window("main") {
                        let url = format!("http://127.0.0.1:{port}");
                        let _ = win.navigate(url.parse().expect("web url"));
                    }
                } else if matches!(last, ServerState::Running | ServerState::Starting) {
                    if let Some(url) = state.console_url.lock().unwrap().clone() {
                        if let Some(win) = app.get_webview_window("main") {
                            let _ = win.navigate(url.parse().expect("console url"));
                        }
                    }
                }
                last = cur_state;
            }
            thread::sleep(Duration::from_millis(800));
        }
    });
}

// ── entry ───────────────────────────────────────────────────────────────────

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_main_window(app);
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_autostart::init(MacosLauncher::LaunchAgent, None))
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .manage(AppState {
            server: DshServer::new(),
            console_url: Mutex::new(None),
            hide_notice_shown: AtomicBool::new(false),
        })
        .invoke_handler(tauri::generate_handler![
            status,
            logs,
            start,
            restart,
            stop,
            info,
            open_browser,
            open_data_dir,
            autostart_enabled,
            autostart_set
        ])
        .setup(|app| {
            let handle = app.handle().clone();

            // remember the console URL so we can navigate back to it
            if let Some(win) = app.get_webview_window("main") {
                if let Ok(url) = win.url() {
                    app.state::<AppState>().console_url.lock().unwrap().replace(url.to_string());
                }
            }

            // bundled runtime location (resources/runtime)
            if let Ok(dir) = app.path().resource_dir() {
                app.state::<AppState>().server.set_resource_dir(dir);
            }

            app.set_menu(menu::build_menu(&handle)?)?;
            menu::build_tray(&handle, handle_menu_action)?;

            let state = app.state::<AppState>();
            spawn_navigation_watcher(handle.clone(), state.inner().clone());

            // auto-start the harness service shortly after the window appears
            let srv = state.server.clone();
            thread::spawn(move || {
                thread::sleep(Duration::from_millis(400));
                let _ = srv.start();
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                // Tray-resident mode: closing the window hides it; only
                // 文件 → 退出 (or the tray) actually exits.
                api.prevent_close();
                let _ = window.hide();
                let app = window.app_handle();
                let state = app.state::<AppState>();
                if state.hide_notice_shown.swap(true, Ordering::Relaxed) {
                    return;
                }
                // one-time hint is shown in the console page footer instead
                // of a system notification; nothing to do here.
            }
        })
        .on_menu_event(|app, event| handle_menu_action(app, event.id.as_ref()))
        .run(tauri::generate_context!())
        .expect("error while running dsh-desktop-client");
}
