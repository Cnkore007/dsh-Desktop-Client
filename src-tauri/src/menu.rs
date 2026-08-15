//! DSh Desktop — native menu and tray.

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Wry};

pub const MENU_OPEN_BROWSER: &str = "open_browser";
pub const MENU_OPEN_DATA_DIR: &str = "open_data_dir";
pub const MENU_RESTART: &str = "restart";
pub const MENU_CONSOLE: &str = "console";
pub const MENU_RELOAD: &str = "reload";
pub const MENU_SHOW_WINDOW: &str = "show_window";
pub const MENU_QUIT: &str = "quit";

pub fn build_menu(app: &AppHandle) -> tauri::Result<Menu<Wry>> {
    let open_browser = MenuItem::with_id(app, MENU_OPEN_BROWSER, "在浏览器打开", true, Some("CmdOrCtrl+Shift+O"))?;
    let open_data = MenuItem::with_id(app, MENU_OPEN_DATA_DIR, "打开数据目录 (~/.dsh)", true, None::<&str>)?;
    let restart = MenuItem::with_id(app, MENU_RESTART, "重启服务", true, Some("CmdOrCtrl+Shift+R"))?;
    let console = MenuItem::with_id(app, MENU_CONSOLE, "控制台", true, Some("CmdOrCtrl+Shift+C"))?;
    let reload = MenuItem::with_id(app, MENU_RELOAD, "重新加载页面", true, Some("CmdOrCtrl+R"))?;
    let quit = MenuItem::with_id(app, MENU_QUIT, "退出", true, Some("CmdOrCtrl+Q"))?;

    let file = Submenu::with_items(app, "文件", true, &[
        &open_browser, &open_data, &restart,
        &PredefinedMenuItem::separator(app)?,
        &quit,
    ])?;
    // Standard macOS Edit menu — without these key equivalents (Cmd+C/V/X/A)
    // keyboard copy/paste is never dispatched to the webview.
    let edit = Submenu::with_items(app, "编辑", true, &[
        &PredefinedMenuItem::undo(app, None)?,
        &PredefinedMenuItem::redo(app, None)?,
        &PredefinedMenuItem::separator(app)?,
        &PredefinedMenuItem::cut(app, None)?,
        &PredefinedMenuItem::copy(app, None)?,
        &PredefinedMenuItem::paste(app, None)?,
        &PredefinedMenuItem::select_all(app, None)?,
    ])?;
    let view = Submenu::with_items(app, "视图", true, &[&console, &reload])?;

    Ok(Menu::with_items(app, &[&file, &edit, &view])?)
}

pub fn build_tray(
    app: &AppHandle,
    on_action: impl Fn(&AppHandle, &str) + Send + Sync + 'static,
) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, MENU_SHOW_WINDOW, "显示窗口", true, None::<&str>)?;
    let open_browser = MenuItem::with_id(app, MENU_OPEN_BROWSER, "在浏览器打开", true, None::<&str>)?;
    let restart = MenuItem::with_id(app, MENU_RESTART, "重启服务", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, MENU_QUIT, "退出", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &open_browser, &restart, &quit])?;

    let on_action = std::sync::Arc::new(on_action);
    let click_action = on_action.clone();
    let mut builder = TrayIconBuilder::with_id("dsh-tray")
        .tooltip("DSh Desktop")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(move |app, event| on_action(app, event.id.as_ref()))
        .on_tray_icon_event(move |tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                click_action(tray.app_handle(), MENU_SHOW_WINDOW);
            }
        });
    if let Some(icon) = app.default_window_icon().cloned() {
        builder = builder.icon(icon);
    }
    builder.build(app)?;
    Ok(())
}
