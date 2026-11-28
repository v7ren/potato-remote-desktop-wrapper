//! Solinear's own tray (menu bar) icon. Closing the window hides it here, so this PC stays reachable; "Quit"
//! really quits. Apollo's own tray icon is compiled out (apollo-patches/README.md), Moonlight has none.
use crate::autostart;
use crate::commands::App;
use std::sync::Arc;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Wry};

/// The tray's two switches, kept so the app can update them when the same settings change in the window.
pub struct Switches {
    pub host: CheckMenuItem<Wry>,
    pub login: CheckMenuItem<Wry>,
}

pub fn show_window(h: &AppHandle) {
    if let Some(w) = h.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

pub fn build(h: &AppHandle, app: Arc<App>, is_host: bool) -> tauri::Result<Switches> {
    let open = MenuItem::with_id(h, "open", "Open Solinear", true, None::<&str>)?;
    let host = CheckMenuItem::with_id(h, "host", "Let my devices stream this PC", true, is_host, None::<&str>)?;
    let login = CheckMenuItem::with_id(h, "login", "Start at login", true, autostart::enabled(), None::<&str>)?;
    let quit = MenuItem::with_id(h, "quit", "Quit Solinear", true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(h)?;
    let menu = Menu::with_items(h, &[&open, &sep, &host, &login, &PredefinedMenuItem::separator(h)?, &quit])?;
    let (host2, login2) = (host.clone(), login.clone());
    TrayIconBuilder::with_id("solinear")
        .icon(h.default_window_icon().cloned().expect("app icon"))
        .tooltip("Solinear")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(move |h, e| match e.id().as_ref() {
            "open" => show_window(h),
            "quit" => h.exit(0),
            "host" => {
                // the menu flipped the check already; do it, then show what's really true
                let (app, host) = (app.clone(), host2.clone());
                std::thread::spawn(move || {
                    let want = host.is_checked().unwrap_or(false);
                    let _ = app.set_hosting(want);
                    let _ = host.set_checked(app.is_host());
                });
            }
            "login" => {
                let (app, login) = (app.clone(), login2.clone());
                std::thread::spawn(move || {
                    let want = login.is_checked().unwrap_or(false);
                    if let Err(e) = autostart::set(want) {
                        app.toast(&format!("Start at login: {e}"));
                    }
                    let _ = login.set_checked(autostart::enabled());
                });
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, e| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e {
                show_window(tray.app_handle());
            }
        })
        .build(h)?;
    Ok(Switches { host, login })
}
