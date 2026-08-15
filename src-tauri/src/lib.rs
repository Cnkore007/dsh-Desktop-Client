//! DeepSeek Harness Desktop Client — Tauri application shell.
//!
//! An original desktop client for DeepSeek Harness: manages the official
//! `dsh web` server lifecycle (spawn/attach/watchdog/logs), provides a
//! branded console page, native clipboard bridging for the web UI, and the
//! standard macOS Edit menu (keyboard copy/paste).
//!
//! Policy: the official DeepSeek Harness sources are NEVER modified. The
//! clipboard bridge that the web UI needs is injected at runtime via a
//! page script (WKWebView `navigator.clipboard.writeText` is unreliable),
//! instead of patching the official frontend bundle.

mod menu;
mod server;

use std::path::PathBuf;
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

/// 剪贴板桥（运行时注入，绝不修改官方 bundle）：
/// 优先 Tauri 原生剪贴板 → `navigator.clipboard.writeText` → execCommand 兜底。
/// 幂等：`__dshBridgeInstalled` 标记避免重复安装。
const CLIPBOARD_BRIDGE_SCRIPT: &str = r#"(()=>{
  if (window.__dshBridgeInstalled) return;
  try {
    var native = window.__TAURI__ && window.__TAURI__.clipboard && window.__TAURI__.clipboard.writeText;
    var orig = navigator.clipboard && navigator.clipboard.writeText;
    if (!native && !orig) { window.__dshBridgeInstalled = true; return; }
    var fn = async function (text) {
      try { if (native) { await native(text); return; } } catch (e) {}
      try { if (orig) { await orig(text); return; } } catch (e) {}
      try {
        var ta = document.createElement('textarea');
        ta.value = text; ta.setAttribute('readonly', '');
        ta.style.position = 'fixed'; ta.style.left = '-9999px';
        document.body.appendChild(ta); ta.select();
        document.execCommand('copy'); ta.remove();
      } catch (e) {}
    };
    if (navigator.clipboard) {
      try { Object.defineProperty(navigator.clipboard, 'writeText', { value: fn, configurable: true }); }
      catch (e) { navigator.clipboard.writeText = fn; }
    }
  } catch (e) {}
  window.__dshBridgeInstalled = true;
})()"#;

/// 定期向主窗口注入剪贴板桥（幂等）。覆盖刷新/手动导航等所有进入页面的路径。
fn spawn_bridge_injector(app: AppHandle) {
    thread::spawn(move || loop {
        thread::sleep(Duration::from_millis(3000));
        if let Some(win) = app.get_webview_window("main") {
            let _ = win.eval(CLIPBOARD_BRIDGE_SCRIPT);
        }
    });
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
fn clear_logs(state: tauri::State<'_, AppState>) {
    state.server.clear_logs();
}

#[tauri::command]
fn start(state: tauri::State<'_, AppState>) -> Result<(), String> {
    let srv = state.server.clone();
    thread::spawn(move || {
        let _ = srv.start();
    });
    Ok(())
}

#[tauri::command]
fn restart(state: tauri::State<'_, AppState>) -> Result<(), String> {
    restart_server(&state.server);
    Ok(())
}

#[tauri::command]
fn stop(state: tauri::State<'_, AppState>) -> Result<(), String> {
    state.server.stop();
    Ok(())
}

#[tauri::command]
fn update_dsh(state: tauri::State<'_, AppState>, version: String) -> Result<(), String> {
    let srv = state.server.clone();
    thread::spawn(move || {
        let _ = srv.update_dsh(&version);
    });
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
    let data_dir = server::dsh_data_dir();
    Info {
        version: env!("CARGO_PKG_VERSION").to_string(),
        dsh_version,
        data_dir: data_dir.display().to_string(),
        port: DshServer::effective_port(),
    }
}

/// 自定义服务端口（持久化到桌面配置并重启服务生效）。
#[tauri::command]
fn set_port(state: tauri::State<'_, AppState>, port: u16) -> Result<(), String> {
    if !(1024..=65535).contains(&port) {
        return Err("端口需在 1024-65535 之间".into());
    }
    if port == DshServer::effective_port() {
        return Ok(());
    }
    server::write_settings_port(port)?;
    restart_server(&state.server);
    Ok(())
}

#[tauri::command]
fn open_browser(app: AppHandle) -> Result<(), String> {
    let port = DshServer::effective_port();
    let url = format!("http://127.0.0.1:{port}");
    app.opener().open_url(url, None::<&str>).map_err(|e| e.to_string())
}

#[tauri::command]
fn open_data_dir(app: AppHandle) -> Result<(), String> {
    let dir = server::dsh_data_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    // 收紧数据目录权限（审计 M13）：避免其他本地用户枚举
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }
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

// ── 界面语言（官方 locale 设置，写 ~/.dsh/settings.yaml）─────────────────────

fn settings_path() -> PathBuf {
    server::dsh_data_dir().join("settings.yaml")
}

fn read_settings_doc() -> serde_yaml::Value {
    std::fs::read_to_string(settings_path())
        .ok()
        .and_then(|t| serde_yaml::from_str(&t).ok())
        .unwrap_or_else(|| serde_yaml::Value::Mapping(Default::default()))
}

fn write_settings_doc(doc: &serde_yaml::Value) -> Result<(), String> {
    let path = settings_path();
    let dir = path.parent().ok_or("settings 路径无父目录")?;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let text = serde_yaml::to_string(doc).map_err(|e| e.to_string())?;
    // 原子写（临时文件 + rename），与官方 dsh-atomic-write 一致，避免半成品
    let tmp = path.with_extension("yaml.tmp");
    std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
    Ok(())
}

/// 读取界面语言偏好："zh" / "en" / "auto"（未设置时跟随浏览器）。
#[tauri::command]
fn get_locale() -> String {
    read_settings_doc()
        .get("locale")
        .and_then(|l| l.get("preference"))
        .and_then(|p| p.as_str())
        .unwrap_or("auto")
        .to_string()
}

/// 写入官方 locale.preference（zh / en / auto）。官方 dsh 通过 chokidar
/// watch settings.yaml，外部修改会被加载；部分界面重启服务后完全生效。
#[tauri::command]
fn set_locale(preference: String) -> Result<(), String> {
    if !matches!(preference.as_str(), "zh" | "en" | "auto") {
        return Err("preference 必须为 zh / en / auto".into());
    }
    let mut doc = read_settings_doc();
    let map = doc
        .as_mapping_mut()
        .ok_or("settings.yaml 顶层不是映射")?;
    let locale = map
        .entry(serde_yaml::Value::String("locale".into()))
        .or_insert_with(|| serde_yaml::Value::Mapping(Default::default()));
    if let serde_yaml::Value::Mapping(lm) = locale {
        lm.insert(
            serde_yaml::Value::String("preference".into()),
            serde_yaml::Value::String(preference.clone()),
        );
    }
    write_settings_doc(&doc)?;
    Ok(())
}

// ── helpers ─────────────────────────────────────────────────────────────────

/// Read the installed dsh package version from the bundled runtime
/// (user-updated runtime first, bundle fallback).
fn dsh_version(server: &DshServer) -> Option<String> {
    let pkg = server
        .runtime_paths_for_version()
        .into_iter()
        .find_map(|base| {
            let p = base.join("node_modules").join("@deepseek-ai").join("dsh").join("package.json");
            p.is_file().then_some(p)
        })?;
    let text = std::fs::read_to_string(pkg).ok()?;
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    json.get("version")?.as_str().map(|s| s.to_string())
}

fn restart_server(server: &DshServer) {
    let srv = server.clone();
    thread::spawn(move || {
        srv.stop();
        thread::sleep(Duration::from_millis(600));
        let _ = srv.start();
    });
}

fn show_main_window(app: &AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.show();
        let _ = win.unminimize();
        let _ = win.set_focus();
    }
}

/// 确保远程页剪贴板写能力覆盖当前端口：默认 3080 由静态 capability 覆盖；
/// 自定义/自动切换的端口通过 dynamic-acl 运行时授权（审计 H6）。
fn ensure_port_capability(app: &AppHandle, port: u16) {
    if port == server::DEFAULT_PORT {
        return;
    }
    use tauri::ipc::CapabilityBuilder;
    let cap = CapabilityBuilder::new("dsh-page-custom")
        .local(false)
        .window("main")
        .remote(format!("http://127.0.0.1:{port}/*"))
        .permission("clipboard-manager:allow-write-text");
    if let Err(e) = app.add_capability(cap) {
        eprintln!("failed to add custom-port capability: {e}");
    }
}

fn navigate_to(app: &AppHandle, url: &str) {
    if let Some(win) = app.get_webview_window("main") {
        if let Ok(parsed) = url.parse() {
            let _ = win.navigate(parsed);
        }
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
        menu::MENU_RESTART => restart_server(&state.server),
        menu::MENU_CONSOLE => {
            if let Some(url) = state.console_url.lock().unwrap().clone() {
                navigate_to(app, &url);
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
/// the dsh web UI accordingly. This is the single navigation authority.
///
/// Hardened per audit: the webview is only navigated to the remote page
/// after the port is confirmed ready (no more ERR_CONNECTION_REFUSED dead
/// pages), and it returns to the console whenever the service leaves the
/// running state.
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
                let port = current
                    .port
                    .unwrap_or_else(|| DshServer::effective_port());
                if cur_state == ServerState::Running {
                    // 就绪探测：服务真正可连后再导航（最多等待 20 秒）
                    let ready = (0..40).any(|_| {
                        thread::sleep(Duration::from_millis(500));
                        state.server.is_ready(port)
                    });
                    if ready {
                        navigate_to(&app, &format!("http://127.0.0.1:{port}"));
                    } else if let Some(url) = state.console_url.lock().unwrap().clone() {
                        navigate_to(&app, &url);
                    }
                } else if matches!(last, ServerState::Running | ServerState::Starting) {
                    if let Some(url) = state.console_url.lock().unwrap().clone() {
                        navigate_to(&app, &url);
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
            clear_logs,
            start,
            restart,
            stop,
            update_dsh,
            info,
            set_port,
            open_browser,
            open_data_dir,
            autostart_enabled,
            autostart_set,
            get_locale,
            set_locale
        ])
        .setup(|app| {
            let handle = app.handle().clone();

            // remember the console URL so we can navigate back to it;
            // the webview may not have committed its first load yet, so
            // capture it shortly after startup as well (audit H11).
            {
                if let Some(win) = app.get_webview_window("main") {
                    if let Ok(url) = win.url() {
                        let s = url.to_string();
                        if !s.is_empty() && s != "about:blank" {
                            app.state::<AppState>()
                                .console_url
                                .lock()
                                .unwrap()
                                .replace(s);
                        }
                    }
                }
            }
            let handle2 = handle.clone();
            thread::spawn(move || {
                thread::sleep(Duration::from_millis(1500));
                if let Some(win) = handle2.get_webview_window("main") {
                    if let Ok(url) = win.url() {
                        let s = url.to_string();
                        if !s.is_empty() && s != "about:blank" {
                            handle2
                                .state::<AppState>()
                                .console_url
                                .lock()
                                .unwrap()
                                .replace(s);
                        }
                    }
                }
            });

            // bundled runtime location (resources/runtime)
            if let Ok(dir) = app.path().resource_dir() {
                app.state::<AppState>().server.set_resource_dir(dir);
            }

            // 端口能力联动：启动时按有效端口授权；运行时自动换端口时动态授权
            ensure_port_capability(&handle, DshServer::effective_port());
            {
                let state = app.state::<AppState>();
                let cb_handle = handle.clone();
                state.server.on_port_change(Box::new(move |port| {
                    let app = cb_handle.clone();
                    thread::spawn(move || ensure_port_capability(&app, port));
                }));
            }

            app.set_menu(menu::build_menu(&handle)?)?;
            menu::build_tray(&handle, handle_menu_action)?;

            let state = app.state::<AppState>();
            spawn_navigation_watcher(handle.clone(), state.inner().clone());
            spawn_bridge_injector(handle.clone());

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
            }
        })
        .on_menu_event(|app, event| handle_menu_action(app, event.id.as_ref()))
        .run(tauri::generate_context!())
        .expect("error while running deepseek-harness-desktop-client");
}
