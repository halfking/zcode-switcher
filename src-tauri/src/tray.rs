//! 系统托盘：窗口关闭后驻留后台，托盘菜单提供「显示主窗口 / 退出」。
//!
//! 跨平台（macOS/Windows/Linux）共用同一份构建逻辑：
//!   - 左键点击托盘图标 → 恢复主窗口（Windows/Linux 习惯）
//!   - 右键点击托盘图标 → 弹出菜单（macOS 习惯；Windows/Linux 右键默认也弹菜单）
//!   - 菜单项「Quit」走 app.exit(0)，保证 Tauri 的清理钩子正常执行
//!
//! 窗口关闭 → 隐藏的拦截在 lib.rs 的 on_window_event 里，与这里配对使用。

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Runtime};

/// 恢复主窗口：显示 + 取消最小化 + 聚焦。
/// unminimize 是给「用户先把窗口最小化到任务栏再从托盘点恢复」的场景兜底。
/// 除托盘外，macOS Dock 图标点击（RunEvent::Reopen）也走这里恢复窗口。
pub(crate) fn restore_main_window<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window("main") {
        let shown = window.show();
        let unminimized = window.unminimize();
        let focused = window.set_focus();
        eprintln!(
            "[restore] main window: show={shown:?} unminimize={unminimized:?} focus={focused:?} visible={:?}",
            window.is_visible()
        );
    } else {
        eprintln!("[restore] main window not found");
    }
}

pub fn setup_tray<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let show_item = MenuItem::with_id(app, "show", "Show ZCode Switcher", true, None::<&str>)?;
    let quit_item = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&show_item, &separator, &quit_item])?;

    let app_for_menu = app.clone();
    let app_for_click = app.clone();

    // TODO(i18n): 托盘菜单文案固定英文。Rust 侧做本地化需要额外的 locale 机制，
    // 现有 i18n 只覆盖前端；后续若需要，可在前端启动后 invoke 一个 set_tray_labels。
    //
    // TODO(icon): 复用 app 默认窗口图标（32x32.png 彩色版）。macOS 上惯例是提供
    // 单色 Template 图标以适配深浅色菜单栏，后续补 tray-icon.png + Template 变体。
    let mut tray = TrayIconBuilder::with_id("zcode-switcher-tray")
        .tooltip("ZCode Switcher")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(move |_app, event| match event.id.as_ref() {
            "show" => restore_main_window(&app_for_menu),
            "quit" => app_for_menu.exit(0),
            _ => {}
        })
        .on_tray_icon_event(move |_tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                restore_main_window(&app_for_click);
            }
        });
    if let Some(icon) = app.default_window_icon().cloned() {
        tray = tray.icon(icon);
    }
    tray.build(app)?;
    Ok(())
}
