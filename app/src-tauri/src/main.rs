#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod admin;
mod agent;
mod apollo;
mod autostart;
mod commands;
mod keys;
mod moonlight;
mod net;
mod overlay;
mod quality;
mod screens;
mod store;
mod tray;

use commands::*;
use std::sync::Arc;
use tauri::Manager;

/// The one place the app's Tauri context is generated: on macOS it embeds Info.plist under a fixed symbol name,
/// so a second generate_context! (e.g. for the toolbar) would not link.
pub fn context() -> tauri::Context {
    tauri::generate_context!()
}

fn main() {
    if std::env::args().nth(1).as_deref() == Some("--overlay") {
        return overlay::run(); // the stream toolbar process
    }
    // hosting needs admin (see admin.rs); nothing is running yet, so there's nothing to clean up
    let hosting = store::load().get("host").and_then(|v| v.as_bool()).unwrap_or(false);
    if hosting && !admin::is_admin() && admin::relaunch_elevated() {
        return;
    }
    // started only now: an elevated relaunch above must not leave a second helper on the same state dir
    let login_server = store::load().get("login_server").and_then(|v| v.as_str()).map(String::from);
    let net: Arc<dyn net::Net> = match net::Helper::start(login_server.as_deref()) {
        Ok(h) => Arc::new(h),
        Err(e) => Arc::new(net::Stub(e)),
    };
    let app = App::new(net);
    let exit = app.clone();
    let setup = app.clone();
    let background = std::env::args().any(|a| a == "--background"); // started at login: straight to the tray
    tauri::Builder::default()
        .manage(app)
        .setup(move |a| {
            let _ = setup.handle.set(a.handle().clone());
            let switches = tray::build(a.handle(), setup.clone(), setup.is_host())?;
            let _ = setup.tray.set(switches);
            if background {
                if let Some(w) = a.get_webview_window("main") {
                    let _ = w.hide();
                }
            }
            Ok(())
        })
        .on_window_event(|w, e| {
            // closing the window keeps Solinear running in the tray (hosting, start at login); Quit is in the tray
            if let tauri::WindowEvent::CloseRequested { api, .. } = e {
                if w.label() == "main" {
                    api.prevent_close();
                    let _ = w.hide();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            state, sign_in, switch_account, set_hosting, hosts, settings, save_settings, play, set_autostart
        ])
        .build(context())
        .expect("failed to build Solinear")
        .run(move |_, e| {
            if let tauri::RunEvent::Exit = e {
                exit.shutdown();
            }
        });
}
