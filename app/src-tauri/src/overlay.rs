//! The stream toolbar, run as a thin second process of this exe (`solinear --overlay`).
//!
//! Design: the session (the Moonlight child) lives in the main process; this process only shows
//! ui/toolbar.html and forwards each toolbar command over a loopback HTTP endpoint of the main process
//! (see `App::ctl_start`), authenticated by a random per-launch token passed in SOLINEAR_CTL=port:token.
//! Why a process and not a second window: on macOS the toolbar has to float over Moonlight's fullscreen Space,
//! which needs NSApplicationActivationPolicyAccessory, and that would hide the main app's Dock icon. One code
//! path for both platforms keeps it simple. It exits by itself once the main app has no stream any more.
use serde_json::{json, Value};
use std::time::Duration;
use tauri::{LogicalSize, Manager, WebviewUrl, WebviewWindowBuilder};

pub const ENV: &str = "SOLINEAR_CTL";
const PILL: (f64, f64) = (245.0, 50.0);

struct Ctl {
    port: u16,
    token: String,
}

fn forward(c: &Ctl, name: &str, args: &Value) -> Result<Value, String> {
    let a: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).timeout_global(Some(Duration::from_secs(60))).build().into();
    let mut r = a.post(format!("http://127.0.0.1:{}/{name}", c.port)).header("X-Token", &c.token)
        .send_json(args).map_err(|e| e.to_string())?;
    let code = r.status().as_u16();
    let v: Value = r.body_mut().read_json().unwrap_or(Value::Null);
    if code == 200 { Ok(v) } else { Err(v["error"].as_str().unwrap_or("The app refused.").into()) }
}

/// What toolbar.js invokes for every command. Window commands are handled here, the rest go to the main app.
#[tauri::command(rename_all = "snake_case")]
async fn fwd(win: tauri::WebviewWindow, ctl: tauri::State<'_, Ctl>, name: String, args: Value) -> Result<Value, String> {
    if name == "toolbar_resize" {
        let (w, h) = (args["w"].as_f64().unwrap_or(PILL.0), args["h"].as_f64().unwrap_or(PILL.1));
        let top_left = win.outer_position().map_err(|e| e.to_string())?; // macOS grows a window from its bottom-left
        win.set_size(LogicalSize::new(w, h)).map_err(|e| e.to_string())?;
        win.set_position(top_left).map_err(|e| e.to_string())?;
        return Ok(json!({"ok": true}));
    }
    let (port, token) = (ctl.port, ctl.token.clone());
    tauri::async_runtime::spawn_blocking(move || forward(&Ctl { port, token }, &name, &args)).await.map_err(|e| e.to_string())?
}

#[cfg(target_os = "macos")]
fn mac_float(win: &tauri::WebviewWindow) {
    // The same NSWindow calls as the Python overlay's _mac_float, through the Objective-C runtime directly.
    use std::ffi::{c_char, c_void};
    #[link(name = "objc")]
    extern "C" {
        fn objc_msgSend();
        fn sel_registerName(name: *const c_char) -> *mut c_void;
    }
    let Ok(w) = win.ns_window() else { return };
    unsafe {
        let set_bool: extern "C" fn(*mut c_void, *mut c_void, i8) = std::mem::transmute(objc_msgSend as *const ());
        let set_int: extern "C" fn(*mut c_void, *mut c_void, isize) = std::mem::transmute(objc_msgSend as *const ());
        set_bool(w, sel_registerName(c"setHidesOnDeactivate:".as_ptr()), 0);
        set_int(w, sel_registerName(c"setLevel:".as_ptr()), 25); // NSStatusWindowLevel: above Moonlight
        set_int(w, sel_registerName(c"setCollectionBehavior:".as_ptr()), 1 | 1 << 4 | 1 << 8); // all Spaces | stationary | fullscreen auxiliary
    }
}

pub fn run() {
    let Some((port, token)) = std::env::var(ENV).ok().and_then(|v| {
        let (p, t) = v.split_once(':')?;
        Some((p.parse().ok()?, t.to_string()))
    }) else { return };
    let ctl = Ctl { port, token };
    // the saved spot, else top-centre of the primary screen
    let saved = forward(&ctl, "toolbar_pos", &json!({})).ok().filter(|v| v["x"].is_number());
    let mut ctx = tauri::generate_context!();
    ctx.config_mut().app.windows.clear(); // no main window here
    tauri::Builder::default()
        .manage(ctl)
        .invoke_handler(tauri::generate_handler![fwd])
        .setup(|app| {
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory); // no Dock icon; may sit over fullscreen apps
            let (x, y) = match saved {
                Some(v) => (v["x"].as_f64().unwrap_or(0.0), v["y"].as_f64().unwrap_or(0.0)),
                None => app.primary_monitor().ok().flatten()
                    .map(|m| ((m.size().width as f64 / m.scale_factor() - PILL.0) / 2.0, 8.0)).unwrap_or((400.0, 8.0)),
            };
            let win = WebviewWindowBuilder::new(app, "toolbar", WebviewUrl::App("toolbar.html".into()))
                .title("Solinear toolbar").inner_size(PILL.0, PILL.1).position(x, y)
                .decorations(false).transparent(true).shadow(false).always_on_top(true).skip_taskbar(true).resizable(false)
                .focused(false).build()?;
            #[cfg(target_os = "macos")]
            mac_float(&win);
            let _ = win;
            // stream ended (or the main app is gone): close
            let h = app.handle().clone();
            std::thread::spawn(move || loop {
                std::thread::sleep(Duration::from_secs(1));
                let c = h.state::<Ctl>();
                if forward(&c, "toolbar_info", &json!({})).is_err() {
                    h.exit(0);
                    break;
                }
            });
            Ok(())
        })
        .build(ctx)
        .expect("failed to build the toolbar")
        .run(|_, _| {});
}
