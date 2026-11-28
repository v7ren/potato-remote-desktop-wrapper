//! Host side: our own streaming server (Apollo on Windows, Sunshine on macOS): its config, login,
//! state file, process, and the local web API used to hand it a client's PIN.
use crate::keys::{keycombos, keymap};
use crate::store::{self, Cfg, MAC};
use serde_json::{json, Value};
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub const BASE_PORT: u16 = 57989; // user's own Sunshine/Apollo uses 47989
pub const WEB_PORT: u16 = BASE_PORT + 1;
pub const AGENT_PORT: u16 = 57980; // below the server's lowest port (BASE_PORT - 5)

/// Name kept from the Python app for existing installs; holds Sunshine's files on macOS too.
pub fn home() -> PathBuf {
    store::data_dir().join("apollo")
}

pub fn server_exe() -> Option<PathBuf> {
    if !MAC {
        return Some(store::bin().join("apollo/sunshine.exe"));
    }
    // shortcut: Mac uses the user's Sunshine install (Homebrew or .dmg); bundle it inside Solinear.app later
    ["/opt/homebrew/bin/sunshine", "/usr/local/bin/sunshine", "/Applications/Sunshine.app/Contents/MacOS/sunshine"]
        .iter().map(PathBuf::from).find(|p| p.exists())
}

fn server_cmd(args: &[&Path]) -> io::Result<Command> {
    let exe = server_exe().ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "streaming server not installed"))?;
    let mut c = Command::new(exe);
    c.args(args);
    if !MAC {
        c.current_dir(store::bin().join("apollo"));
    }
    Ok(c)
}

fn str_of<'a>(cfg: &'a Cfg, k: &str) -> &'a str {
    cfg.get(k).and_then(Value::as_str).unwrap_or("")
}

pub fn conf_text(home: &Path, cfg: &Cfg, mac: bool) -> String {
    let h = |f: &str| home.join(f).display().to_string();
    let mut lines = vec![
        format!("port = {BASE_PORT}"),
        "origin_web_ui_allowed = pc".into(), // web API reachable from this PC only
        format!("file_apps = {}", h("apps.json")),
        format!("file_state = {}", h("state.json")),
        format!("credentials_file = {}", h("state.json")),
        format!("pkey = {}", h("key.pem")),
        format!("cert = {}", h("cert.pem")),
        format!("log_path = {}", h("apollo.log")),
    ];
    if !mac {
        // Windows display-device options (match the client's resolution/refresh)
        lines.push("dd_resolution_option = auto".into());
        lines.push("dd_refresh_rate_option = auto".into());
        lines.push("system_tray = false".into()); // our patched Apollo: Solinear's tray icon is the only one
    }
    let out = str_of(cfg, "output_name");
    if !out.is_empty() {
        lines.push(format!("output_name = {out}"));
    }
    let keys = keymap(str_of(cfg, "keys_for"), mac);
    if !keys.is_empty() {
        let k: Vec<String> = keys.iter().map(|k| format!("0x{k:X}")).collect();
        lines.push(format!("keybindings = [{}]", k.join(", ")));
    }
    let combos = keycombos(str_of(cfg, "keys_for"), mac);
    if !combos.is_empty() {
        let k: Vec<String> = combos.iter().map(|k| format!("0x{k:X}")).collect();
        lines.push(format!("keycombos = [{}]", k.join(", "))); // ignored by an unpatched Apollo
    }
    lines.join("\n") + "\n"
}

fn write_conf(home: &Path, cfg: &Cfg, mac: bool) -> io::Result<()> {
    std::fs::create_dir_all(home)?;
    std::fs::write(home.join("sunshine.conf"), conf_text(home, cfg, mac))
}

/// Rewrite the config for this client's keyboard. True if it changed (the server must restart to read it).
/// The caller saves cfg.
pub fn set_keys(home: &Path, cfg: &mut Cfg, client_os: &str, mac: bool) -> io::Result<bool> {
    if keymap(client_os, mac) == keymap(str_of(cfg, "keys_for"), mac) {
        return Ok(false);
    }
    cfg.insert("keys_for".into(), client_os.into());
    write_conf(home, cfg, mac)?;
    Ok(true)
}

/// Pick which screen the server captures. True if it changed (restart needed). The caller saves cfg.
pub fn set_screen(home: &Path, cfg: &mut Cfg, id: &str, mac: bool) -> io::Result<bool> {
    if str_of(cfg, "output_name") == id {
        return Ok(false);
    }
    cfg.insert("output_name".into(), id.into());
    write_conf(home, cfg, mac)?;
    Ok(true)
}

/// Sunshine/Apollo only save their host ID after the first successful pairing, and make a new random one on
/// every start until then. Each new ID leaves clients' Moonlight with a stale entry for the same address,
/// which later breaks connecting. Write one fixed ID up front, and restore it if the state file is wiped.
pub fn pin_server_id(home: &Path, cfg: &mut Cfg) -> io::Result<()> {
    let path = home.join("state.json");
    let mut data: Value = match std::fs::read_to_string(&path) {
        Ok(t) => serde_json::from_str(&t).unwrap_or_else(|_| json!({})),
        Err(e) if e.kind() == io::ErrorKind::NotFound => json!({}),
        Err(e) => return Err(e),
    };
    let root = data.as_object_mut().ok_or_else(|| io::Error::other("state.json is not an object"))?
        .entry("root").or_insert_with(|| json!({}));
    // `--creds` rewrites this file with every value as a string ("true", "119480064"), and Apollo then
    // crashes reading its paired devices ("type must be boolean, but is string"). Put the types back.
    for dev in root["named_devices"].as_array_mut().into_iter().flatten().filter_map(Value::as_object_mut) {
        for (k, v) in dev.iter_mut() {
            match v.as_str() {
                Some("true") => *v = true.into(),
                Some("false") => *v = false.into(),
                Some(s) if k == "perm" && !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) => {
                    if let Ok(n) = s.parse::<u64>() {
                        *v = n.into();
                    }
                }
                _ => {}
            }
        }
    }
    match root.get("uniqueid").and_then(Value::as_str).filter(|s| !s.is_empty()) {
        Some(id) => {
            cfg.insert("server_id".into(), id.into());
        }
        None => {
            let id = cfg.entry("server_id").or_insert_with(|| uuid::Uuid::new_v4().to_string().to_uppercase().into());
            root["uniqueid"] = id.clone();
        }
    }
    std::fs::create_dir_all(home)?;
    std::fs::write(path, serde_json::to_string_pretty(&data)?)
}

/// First-time host setup. Idempotent. Re-applied on every start: if the server's state file is reset or
/// corrupted, its login goes with it.
pub fn setup(cfg: &mut Cfg) -> io::Result<()> {
    let home = home();
    write_conf(&home, cfg, MAC)?;
    let apps = home.join("apps.json");
    if !apps.exists() {
        std::fs::write(&apps, json!({"env": {}, "apps": [{"name": "Desktop", "image-path": "desktop.png"}]}).to_string())?;
    }
    if !cfg.contains_key("apollo_user") {
        use rand::{distributions::Alphanumeric, Rng};
        cfg.insert("apollo_user".into(), "solinear".into());
        let pass: String = rand::thread_rng().sample_iter(Alphanumeric).take(32).map(char::from).collect();
        cfg.insert("apollo_pass".into(), pass.into());
    }
    let conf = home.join("sunshine.conf");
    let mut cmd = server_cmd(&[&conf])?;
    cmd.args(["--creds", str_of(cfg, "apollo_user"), str_of(cfg, "apollo_pass")]);
    let out = store::quiet(&mut cmd).output()?;
    if !out.status.success() {
        return Err(io::Error::other("streaming server refused its login setup"));
    }
    pin_server_id(&home, cfg)?;
    cfg.insert("host".into(), true.into());
    store::save(cfg)
}

/// The running server process, kept alive: Apollo/Sunshine can exit on their own (e.g. started while the PC
/// is locked: no display to capture), which would leave the host "not answering".
#[derive(Default)]
pub struct Server {
    child: Mutex<Option<Child>>,
    watching: AtomicBool,
}

impl Server {
    fn spawn() -> io::Result<Child> {
        let conf = home().join("sunshine.conf");
        let mut c = server_cmd(&[&conf])?;
        // its log already goes to log_path; don't echo it into the user's terminal
        store::quiet(&mut c).stdout(Stdio::null()).stderr(Stdio::null()).spawn()
    }

    /// Starts it, and from then on starts it again whenever it exits (until `stop`).
    pub fn start(self: &Arc<Self>) -> io::Result<()> {
        *self.child.lock().unwrap() = Some(Self::spawn()?);
        if !self.watching.swap(true, Ordering::SeqCst) {
            let me = self.clone();
            std::thread::spawn(move || loop {
                std::thread::sleep(Duration::from_secs(5));
                let mut g = me.child.lock().unwrap();
                if g.as_mut().is_some_and(|c| c.try_wait().ok().flatten().is_some()) {
                    *g = Self::spawn().ok();
                }
            });
        }
        Ok(())
    }

    /// Its ports must be free before the new one starts, so wait for the old one to be gone.
    pub fn restart(&self) -> io::Result<()> {
        let mut g = self.child.lock().unwrap();
        if let Some(mut c) = g.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
        *g = Some(Self::spawn()?);
        Ok(())
    }

    pub fn stop(&self) {
        if let Some(mut c) = self.child.lock().unwrap().take() {
            let _ = c.kill();
        }
    }
}

/// Host actions the toolbar can trigger. Fixed commands only; the client just names one (see agent::ACTIONS).
pub fn run_action(name: &str) -> bool {
    let a: &[&str] = match (MAC, name) {
        (false, "lock") => &["rundll32.exe", "user32.dll,LockWorkStation"],
        (false, "signout") => &["shutdown", "/l"],
        (false, "restart") => &["shutdown", "/r", "/t", "0"],
        (false, "shutdown") => &["shutdown", "/s", "/t", "0"],
        (false, "explorer") => &["explorer.exe"],
        (false, "taskmgr") => &["taskmgr.exe"],
        (false, "control") => &["control.exe"],
        (true, "lock") => &["pmset", "displaysleepnow"], // locks when "require password after sleep" is on (macOS default)
        (true, "signout") => &["osascript", "-e", "tell application \"System Events\" to log out"],
        (true, "restart") => &["osascript", "-e", "tell application \"System Events\" to restart"],
        (true, "shutdown") => &["osascript", "-e", "tell application \"System Events\" to shut down"],
        (true, "explorer") => &["open", "-a", "Finder"],
        (true, "taskmgr") => &["open", "-a", "Activity Monitor"],
        (true, "control") => &["open", "-a", "System Settings"],
        _ => return false,
    };
    store::quiet(Command::new(a[0]).args(&a[1..])).spawn().is_ok()
}

/// Sunshine: the pending pairing request that came from this client's address (may be IPv6-mapped).
pub fn pick_pairing(pairings: &[Value], client_ip: &str) -> Option<String> {
    pairings.iter().find(|p| p["address"].as_str().unwrap_or("").trim_start_matches("::ffff:") == client_ip)
        .and_then(|p| p["id"].as_str().map(String::from))
}

fn web() -> ureq::Agent {
    // the server's own self-signed cert, localhost only
    let tls = ureq::tls::TlsConfig::builder().disable_verification(true).build();
    ureq::Agent::config_builder().tls_config(tls).timeout_global(Some(Duration::from_secs(5))).build().into()
}

fn basic(cfg: &Cfg) -> String {
    use std::fmt::Write;
    let raw = format!("{}:{}", str_of(cfg, "apollo_user"), str_of(cfg, "apollo_pass"));
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::from("Basic ");
    for c in raw.as_bytes().chunks(3) {
        let n = c.iter().enumerate().fold(0u32, |a, (i, b)| a | (*b as u32) << (16 - 8 * i));
        for i in 0..4 {
            if i <= c.len() {
                let _ = s.write_char(T[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                s.push('=');
            }
        }
    }
    s
}

fn url(path: &str) -> String {
    format!("https://127.0.0.1:{WEB_PORT}{path}")
}

fn pending(cfg: &Cfg) -> Result<Vec<Value>, ureq::Error> {
    let mut r = web().get(url("/api/pin")).header("Authorization", basic(cfg)).call()?;
    Ok(r.body_mut().read_json::<Value>()?["pairings"].as_array().cloned().unwrap_or_default())
}

/// Sunshine keeps an unanswered pairing request open for 5 min and refuses a new one from the same device
/// (Moonlight error 409). Clear this client's stale request before it tries again.
pub fn cancel_pending(cfg: &Cfg, client_ip: &str) {
    if !MAC {
        return; // Apollo has no such lock
    }
    while let Some(id) = pending(cfg).ok().and_then(|p| pick_pairing(&p, client_ip)) {
        let r = web().delete(url("/api/pin")).header("Authorization", basic(cfg)).force_send_body()
            .header("Content-Type", "application/json").send_json(json!({"pairing_id": id}));
        if r.is_err() {
            break;
        }
    }
}

/// Apollo: the session cookie from /api/login.
fn login(cfg: &Cfg) -> Result<String, ureq::Error> {
    let r = web().post(url("/api/login")).send_json(json!({"username": str_of(cfg, "apollo_user"), "password": str_of(cfg, "apollo_pass")}))?;
    Ok(r.headers().get("set-cookie").and_then(|v| v.to_str().ok()).unwrap_or("").split(';').next().unwrap_or("").to_string())
}

/// Apollo gives the first paired client every permission and later ones only the default (no launch, no input),
/// so a second device pairs and then gets "403 Permission denied". 119480064 = 0x071F1F00 = all.
const PERM_ALL: u64 = 119_480_064;

/// The body for /api/clients/update, or None if the client already has everything. The endpoint overwrites
/// every field (omitted do/undo become empty, omitted perm becomes 0), so send the whole object back.
pub fn full_perm_body(client: &Value) -> Option<Value> {
    if client["perm"].as_u64() == Some(PERM_ALL) {
        return None;
    }
    let mut c = client.clone();
    c["perm"] = PERM_ALL.into();
    Some(c)
}

/// Apply the current key map to the running server without a restart (our patched Apollo's /api/input-remap).
/// False if the server can't (unpatched or Mac Sunshine): the caller restarts it to read the config instead.
pub fn apply_keys_live(cfg: &Cfg) -> bool {
    if MAC {
        return false;
    }
    let os = str_of(cfg, "keys_for");
    let body = json!({"keybindings": keymap(os, false), "keycombos": keycombos(os, false)});
    let go = || -> Result<bool, ureq::Error> {
        let cookie = login(cfg)?;
        let mut r = web().post(url("/api/input-remap")).header("Cookie", &cookie).send_json(body)?;
        Ok(r.body_mut().read_json::<Value>()?["status"] == true)
    };
    go().unwrap_or(false)
}

/// Give every paired device full permissions (Apollo only). Best effort: the server may be mid-restart.
pub fn grant_all_perms(cfg: &Cfg) {
    if MAC {
        return; // Sunshine has no per-client permissions
    }
    let go = || -> Result<(), ureq::Error> {
        let cookie = login(cfg)?;
        let mut r = web().get(url("/api/clients/list")).header("Cookie", &cookie).call()?;
        let list = r.body_mut().read_json::<Value>()?;
        for body in list["named_certs"].as_array().into_iter().flatten().filter_map(full_perm_body) {
            web().post(url("/api/clients/update")).header("Cookie", &cookie).send_json(body)?;
        }
        Ok(())
    };
    let _ = go();
}

/// Hand a client's PIN to our server, the same as typing it into its web page.
pub fn submit_pin(cfg: &Cfg, pin: &str, name: &str, client_ip: &str) -> bool {
    let go = || -> Result<bool, ureq::Error> {
        let (auth, body) = if MAC {
            // upstream Sunshine: Basic auth, and the PIN must name the pending request's id
            let Some(id) = pick_pairing(&pending(cfg)?, client_ip) else { return Ok(false) }; // client retries
            (("Authorization", basic(cfg)), json!({"pairing_id": id, "pin": pin, "name": name}))
        } else {
            (("Cookie", login(cfg)?), json!({"pin": pin, "name": name}))
        };
        let mut r = web().post(url("/api/pin")).header(auth.0, auth.1).send_json(body)?;
        Ok(r.body_mut().read_json::<Value>()?["status"].as_bool().unwrap_or(false))
    };
    go().unwrap_or(false) // server down, wrong creds, or no pairing pending yet
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> PathBuf {
        let d = std::env::temp_dir().join(format!("solinear-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn state(home: &Path) -> Value {
        serde_json::from_str(&std::fs::read_to_string(home.join("state.json")).unwrap()).unwrap()
    }

    #[test]
    fn adds_id_keeps_creds() {
        let h = tmp();
        std::fs::write(h.join("state.json"), r#"{"username":"solinear","salt":"x","password":"y"}"#).unwrap();
        let mut cfg = Cfg::new();
        pin_server_id(&h, &mut cfg).unwrap();
        let d = state(&h);
        assert_eq!(d["root"]["uniqueid"], cfg["server_id"]);
        assert_eq!(d["username"], "solinear");
    }

    #[test]
    fn stable_and_restored_after_wipe() {
        let h = tmp();
        let mut cfg = Cfg::new();
        pin_server_id(&h, &mut cfg).unwrap();
        let first = cfg["server_id"].clone();
        pin_server_id(&h, &mut cfg).unwrap();
        std::fs::remove_file(h.join("state.json")).unwrap();
        pin_server_id(&h, &mut cfg).unwrap();
        assert_eq!(state(&h)["root"]["uniqueid"], first);
    }

    #[test]
    fn stringified_device_fields_restored() {
        // what `sunshine.exe --creds` leaves behind once devices are paired
        let h = tmp();
        let dev = json!({"name": "Mac", "uuid": "U1", "cert": "C", "display_mode": "", "perm": "119480064",
                         "allow_client_commands": "true", "always_use_virtual_display": "false"});
        std::fs::write(h.join("state.json"), json!({"root": {"uniqueid": "ABC-1", "named_devices": [dev]}}).to_string()).unwrap();
        pin_server_id(&h, &mut Cfg::new()).unwrap();
        let d = &state(&h)["root"]["named_devices"][0];
        assert_eq!(d["allow_client_commands"], Value::Bool(true));
        assert_eq!(d["always_use_virtual_display"], Value::Bool(false));
        assert_eq!(d["perm"], json!(119480064));
        assert_eq!((d["name"].as_str(), d["display_mode"].as_str()), (Some("Mac"), Some("")));
    }

    #[test]
    fn existing_server_id_adopted() {
        let h = tmp();
        std::fs::write(h.join("state.json"), json!({"root": {"uniqueid": "ABC-1", "named_devices": []}}).to_string()).unwrap();
        let mut cfg = Cfg::new();
        pin_server_id(&h, &mut cfg).unwrap();
        assert_eq!(cfg["server_id"], "ABC-1");
    }

    #[test]
    fn restart_only_when_mapping_changes() {
        let h = tmp();
        let conf = || std::fs::read_to_string(h.join("sunshine.conf")).unwrap();
        let mut cfg = Cfg::new();
        assert!(!set_keys(&h, &mut cfg, "windows", false).unwrap()); // same OS: nothing to do
        assert!(set_keys(&h, &mut cfg, "mac", false).unwrap());
        assert!(conf().contains("keybindings = [0x5B, 0xA2,"));
        assert!(conf().contains("keycombos = [0x5B, 0x9, 0xA4]")); // Cmd+Tab -> Alt+Tab
        assert!(!set_keys(&h, &mut cfg, "mac", false).unwrap());
        assert!(set_keys(&h, &mut cfg, "", false).unwrap()); // toggle off: back to no remap
        assert!(!conf().contains("keybindings"));
    }

    #[test]
    fn screen_choice_written_and_kept_with_keys() {
        let h = tmp();
        let mut cfg = Cfg::new();
        assert!(set_screen(&h, &mut cfg, r"\\.\DISPLAY2", false).unwrap());
        assert!(!set_screen(&h, &mut cfg, r"\\.\DISPLAY2", false).unwrap());
        set_keys(&h, &mut cfg, "mac", false).unwrap(); // rewriting for keys must not lose the screen
        let c = std::fs::read_to_string(h.join("sunshine.conf")).unwrap();
        assert!(c.contains(r"output_name = \\.\DISPLAY2") && c.contains("keybindings"));
        assert!(!conf_text(&h, &Cfg::new(), false).contains("output_name"));
    }

    #[test]
    fn pairing_pick_matches_ip_including_ipv6_mapped() {
        let pend = vec![json!({"id": "a", "address": "100.1.1.1"}), json!({"id": "b", "address": "::ffff:100.2.2.2"})];
        assert_eq!(pick_pairing(&pend, "100.2.2.2").as_deref(), Some("b"));
        assert_eq!(pick_pairing(&[json!({"id": "a", "address": "100.9.9.9"})], "100.2.2.2"), None);
    }

    #[test]
    fn perm_update_sends_whole_object_with_full_perm() {
        let c = json!({"name": "Mac", "uuid": "U", "display_mode": "", "perm": 50331648, "do": [{"cmd": "x"}],
                       "undo": [], "enable_legacy_ordering": true, "allow_client_commands": true,
                       "always_use_virtual_display": false});
        let b = full_perm_body(&c).unwrap();
        let mut want = c.clone();
        want["perm"] = json!(119480064);
        assert_eq!(b, want); // nothing else touched, do/undo kept
        assert!(full_perm_body(&b).is_none());
    }

    #[test]
    fn basic_auth_header() {
        let cfg: Cfg = serde_json::from_value(json!({"apollo_user": "solinear", "apollo_pass": "pw"})).unwrap();
        assert_eq!(basic(&cfg), "Basic c29saW5lYXI6cHc="); // base64("solinear:pw")
    }
}
