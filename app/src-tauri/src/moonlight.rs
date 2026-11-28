//! Client side: find the owner's hosts, pair with them, start the stream. Moonlight's own UI never opens.
use crate::apollo::{AGENT_PORT, BASE_PORT};
use crate::screens::Screen;
use crate::store::{self, Cfg, MAC};
use crate::quality;
use serde_json::{json, Value};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

pub fn exe() -> PathBuf {
    if MAC {
        // shortcut: Mac uses the installed Moonlight.app and shares its settings; bundle it inside Solinear.app later
        PathBuf::from("/Applications/Moonlight.app/Contents/MacOS/Moonlight")
    } else {
        store::bin().join("moonlight/Moonlight.exe")
    }
}

/// Portable mode looks for its settings in the cwd, so Moonlight always runs with cwd = its folder.
fn cwd() -> Option<PathBuf> {
    (!MAC).then(|| store::bin().join("moonlight"))
}

fn addr(ip: &str) -> String {
    format!("{ip}:{BASE_PORT}")
}

pub fn list_args(ip: &str) -> Vec<String> {
    vec!["list".into(), addr(ip)]
}

pub fn pair_args(ip: &str, pin: &str) -> Vec<String> {
    vec!["pair".into(), addr(ip), "--pin".into(), pin.into()]
}

pub fn stream_args(ip: &str, flags: Vec<String>) -> Vec<String> {
    ["stream".to_string(), addr(ip), "Desktop".into()].into_iter().chain(flags).collect()
}

fn command(args: &[String]) -> Command {
    let mut c = Command::new(exe());
    c.args(args);
    if let Some(d) = cwd() {
        let _ = std::fs::write(d.join("portable.dat"), "");
        c.current_dir(d);
    }
    c
}

// --- talking to a host's agent ------------------------------------------------------------------------------
fn agent() -> ureq::Agent {
    ureq::Agent::config_builder().http_status_as_error(false).timeout_global(Some(Duration::from_secs(20))).build().into()
}

/// (status code, body). Err = host unreachable.
pub fn agent_post(ip: &str, path: &str, body: Value) -> Result<(u16, Value), String> {
    let mut r = agent().post(format!("http://{ip}:{AGENT_PORT}{path}")).send_json(body).map_err(|e| e.to_string())?;
    Ok((r.status().as_u16(), r.body_mut().read_json().unwrap_or(Value::Null)))
}

pub fn agent_info(ip: &str) -> Option<Value> {
    let a: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_millis(1500))).build().into();
    a.get(format!("http://{ip}:{AGENT_PORT}/info")).call().ok()?.body_mut().read_json().ok()
}

// --- pairing ------------------------------------------------------------------------------------------------
pub fn is_paired(ip: &str) -> bool {
    // moonlight's pair/stream always exit 0; `list` is the only honest check
    let mut c = command(&list_args(ip));
    let Ok(mut child) = store::quiet(&mut c).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn() else { return false };
    let end = Instant::now() + Duration::from_secs(40);
    while Instant::now() < end {
        match child.try_wait() {
            Ok(Some(s)) => return s.success(),
            Ok(None) => std::thread::sleep(Duration::from_millis(200)),
            Err(_) => return false,
        }
    }
    let _ = child.kill();
    false
}

/// The host's streaming server takes a few seconds to (re)start; don't hand Moonlight a closed port.
pub fn wait_for_server(ip: &str, timeout: Duration) -> bool {
    let Ok(sa) = format!("{ip}:{BASE_PORT}").parse::<SocketAddr>() else { return false };
    let end = Instant::now() + timeout;
    while Instant::now() < end {
        if TcpStream::connect_timeout(&sa, Duration::from_secs(2)).is_ok() {
            return true;
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    false
}

/// Ask the host to remap keys for this device's keyboard (Mac Cmd+C -> Ctrl+C on Windows, and back). The host
/// restarts its server when the mapping changes, so wait for it before Moonlight connects.
pub fn match_keys(ip: &str, on: bool) {
    let client_os = if !on { "" } else if MAC { "mac" } else { "windows" };
    if let Ok((200, v)) = agent_post(ip, "/keys", json!({"client_os": client_os})) {
        if v["restarted"] == true {
            std::thread::sleep(Duration::from_secs(1));
            wait_for_server(ip, Duration::from_secs(20));
        }
    } // older host without /keys: stream without the remap
}

pub fn pair(ip: &str) -> Result<bool, String> {
    let pin = format!("{:04}", rand::random::<u32>() % 10_000);
    let _ = agent_post(ip, "/cancel", json!({})); // a Sunshine host refuses a new pairing (409) while an old one is pending
    let mut c = command(&pair_args(ip, &pin));
    let mut child = c.spawn().map_err(|e| e.to_string())?;
    let end = Instant::now() + Duration::from_secs(30); // room for a first-time macOS "Allow incoming connections?" prompt
    let mut ok = false;
    let mut res = Ok(());
    while Instant::now() < end && matches!(child.try_wait(), Ok(None)) && !ok {
        std::thread::sleep(Duration::from_millis(500)); // the server rejects the PIN until moonlight's request is pending
        match agent_post(ip, "/pair", json!({"pin": pin, "name": hostname()})) {
            Ok((200, v)) => ok = v["status"] == true,
            Ok((403, _)) => {
                res = Err("That PC belongs to a different account.".to_string());
                break;
            }
            _ => {}
        }
    }
    if ok {
        let end = Instant::now() + Duration::from_secs(15); // success quits by itself
        while Instant::now() < end && matches!(child.try_wait(), Ok(None)) {
            std::thread::sleep(Duration::from_millis(200));
        }
    } else {
        let _ = agent_post(ip, "/cancel", json!({})); // don't leave a request that blocks the next try for 5 min
    }
    let _ = child.kill();
    res?;
    Ok(is_paired(ip))
}

fn hostname() -> String {
    std::env::var("COMPUTERNAME").or_else(|_| std::env::var("HOSTNAME")).unwrap_or_else(|_| "device".into())
}

// --- streaming ----------------------------------------------------------------------------------------------
#[derive(Clone, Debug, PartialEq)]
pub struct Opts {
    pub quality: String,
    pub display_mode: String,
    pub reverse_scroll: bool,
    pub hdr: bool,
    pub size: (u32, u32),
    pub fps: u32,
}

/// Defaults from saved settings and this screen, streaming the host's `screen` at its own shape (see quality::fit).
pub fn default_opts(cfg: &Cfg, screen: Option<&Screen>) -> Opts {
    let (cw, ch, fps) = quality::screen();
    Opts {
        quality: cfg.get("quality").and_then(Value::as_str).unwrap_or("auto").into(),
        display_mode: cfg.get("display_mode").and_then(Value::as_str).unwrap_or("borderless").into(),
        reverse_scroll: cfg.get("reverse_scroll").and_then(Value::as_bool).unwrap_or(false),
        hdr: false,
        size: screen.map_or((cw, ch), |s| quality::fit(s.w, s.h, cw, ch)),
        fps,
    }
}

pub fn stream(ip: &str, o: &Opts, direct: bool) -> std::io::Result<Child> {
    let kbps = quality::bitrate_kbps(o.size.0, o.size.1, o.fps, direct, &o.quality);
    let flags = quality::flags(o.size.0, o.size.1, o.fps, kbps, &o.display_mode, o.reverse_scroll, o.hdr);
    command(&stream_args(ip, flags)).spawn()
}

/// "f1".."f12" -> 0..11
fn f_index(key: &str) -> Option<usize> {
    let n: usize = key.strip_prefix('f')?.parse().ok()?;
    (1..=12).contains(&n).then(|| n - 1)
}

/// Focus the stream window and press MOD+key, as if typed. Moonlight handles X/M/S/Q itself; the host's server
/// switches the captured screen on F1..F12.
#[cfg(windows)]
pub fn send_combo(pid: u32, key: &str) -> Result<(), String> {
    use windows::core::BOOL;
    use windows::Win32::Foundation::{HWND, LPARAM};
    use windows::Win32::UI::Input::KeyboardAndMouse::*;
    use windows::Win32::UI::WindowsAndMessaging::*;

    let scan = match (key, f_index(key)) {
        ("x", _) => 0x2D, ("m", _) => 0x32, ("s", _) => 0x1F, ("q", _) => 0x10,
        (_, Some(n)) => [0x3B, 0x3C, 0x3D, 0x3E, 0x3F, 0x40, 0x41, 0x42, 0x43, 0x44, 0x57, 0x58][n], // F1..F12
        _ => return Err("unknown key".into()),
    };
    unsafe extern "system" fn find(h: HWND, data: LPARAM) -> BOOL {
        let d = &mut *(data.0 as *mut (u32, Option<HWND>));
        let mut owner = 0;
        GetWindowThreadProcessId(h, Some(&mut owner));
        if owner == d.0 && IsWindowVisible(h).as_bool() {
            d.1 = Some(h);
            return false.into();
        }
        true.into()
    }
    let mut found = (pid, None);
    unsafe {
        let _ = EnumWindows(Some(find), LPARAM(&mut found as *mut _ as isize));
        if let Some(h) = found.1 {
            let _ = SetForegroundWindow(h);
        }
    }
    std::thread::sleep(Duration::from_millis(50));
    let mods = [0x1D, 0x38, 0x2A]; // Ctrl, Alt, Shift scancodes, which SDL/Moonlight read
    let seq: Vec<(u16, bool)> = mods.iter().map(|m| (*m, false)).chain([(scan, false), (scan, true)])
        .chain(mods.iter().rev().map(|m| (*m, true))).collect();
    let inputs: Vec<INPUT> = seq.iter().map(|(sc, up)| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 { ki: KEYBDINPUT {
            wScan: *sc, dwFlags: KEYEVENTF_SCANCODE | if *up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) },
            ..Default::default() } },
    }).collect();
    if unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) } as usize == inputs.len() { Ok(()) } else { Err("input blocked".into()) }
}

#[cfg(not(windows))]
pub fn send_combo(_pid: u32, key: &str) -> Result<(), String> {
    // needs Accessibility permission for the app running Solinear
    let code = match (key, f_index(key)) {
        ("x", _) => 7, ("m", _) => 46, ("s", _) => 1, ("q", _) => 12,
        (_, Some(n)) => [122, 120, 99, 118, 96, 97, 98, 100, 101, 109, 103, 111][n], // F1..F12
        _ => return Err("unknown key".into()),
    };
    Command::new("osascript").args(["-e", "tell application \"Moonlight\" to activate", "-e",
        &format!("tell application \"System Events\" to key code {code} using {{control down, option down, shift down}}")])
        .output().map(|_| ()).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f_keys() {
        assert_eq!((f_index("f1"), f_index("f12"), f_index("f13"), f_index("f0"), f_index("x")), (Some(0), Some(11), None, None, None));
    }

    #[test]
    fn argv() {
        assert_eq!(list_args("100.1.2.3"), ["list", "100.1.2.3:57989"]);
        assert_eq!(pair_args("100.1.2.3", "0420"), ["pair", "100.1.2.3:57989", "--pin", "0420"]);
        let s = stream_args("100.1.2.3", quality::flags(1920, 1080, 60, 20000, "borderless", false, false));
        assert_eq!(&s[..4], ["stream", "100.1.2.3:57989", "Desktop", "--resolution"]);
        assert!(s.contains(&"--absolute-mouse".to_string()));
    }

    #[test]
    fn opts_follow_host_screen_shape() {
        let host = Screen { id: "x".into(), name: "x".into(), w: 1000, h: 1000, primary: true };
        let o = default_opts(&Cfg::new(), Some(&host));
        assert!(o.size.0 == o.size.1 && o.size.0 <= 1000);
        assert_eq!((o.quality.as_str(), o.display_mode.as_str()), ("auto", "borderless"));
    }
}
