#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod admin;
mod agent;
mod apollo;
mod commands;
mod keys;
mod moonlight;
mod net;
mod overlay;
mod quality;
mod screens;
mod store;

use commands::*;
use std::sync::Arc;

fn main() {
    if std::env::args().nth(1).as_deref() == Some("--overlay") {
        return overlay::run(); // the stream toolbar process
    }
    // hosting needs admin (see admin.rs); nothing is running yet, so there's nothing to clean up
    let hosting = store::load().get("host").and_then(|v| v.as_bool()).unwrap_or(false);
    if hosting && !admin::is_admin() && admin::relaunch_elevated() {
        return;
    }
    // the real network helper replaces net::Stub later
    let app = App::new(Arc::new(net::Stub));
    let exit = app.clone();
    let setup = app.clone();
    tauri::Builder::default()
        .manage(app)
        .setup(move |a| {
            let _ = setup.handle.set(a.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            state, sign_in, switch_account, set_hosting, hosts, settings, save_settings, play
        ])
        .build(tauri::generate_context!())
        .expect("failed to build Solinear")
        .run(move |_, e| {
            if let tauri::RunEvent::Exit = e {
                exit.shutdown();
            }
        });
}
