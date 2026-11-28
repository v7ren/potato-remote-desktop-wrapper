//! The Tauri commands (see CONTRACT.md) and the app state they share. Everything network-dependent goes
//! through `net::Net`.
use crate::admin;
use crate::agent::{self, Agent, Hooks};
use crate::apollo;
use crate::moonlight::{self, Opts};
use crate::net::{Net, Status};
use crate::screens::{self, Screen};
use crate::store::{self, Cfg, MAC};
use crate::quality;
use serde_json::{json, Value};
use std::process::{Child, Command};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use tauri::Emitter;

struct Session {
    id: u64,
    ip: String,
    /// Where Moonlight connects (the helper's local forwarder to `ip`).
    addr: String,
    name: String,
    os: String,
    screens: Vec<Screen>,
    current_screen: String,
    opts: Opts,
    child: Child,
    /// Patched Moonlight's control channel; None for the Mac's stock Moonlight.app.
    ctl: Option<moonlight::Ctl>,
}

pub struct App {
    cfg: Mutex<Cfg>,
    net: Arc<dyn Net>,
    server: Arc<apollo::Server>,
    agent: Mutex<Option<agent::Running>>,
    session: Mutex<Option<Session>>,
    /// The toolbar process, and the loopback endpoint (port, token) it talks to.
    overlay: Mutex<Option<Child>>,
    ctl: OnceLock<(u16, String)>,
    next_id: AtomicU64,
    pub handle: OnceLock<tauri::AppHandle>,
}

type R = Result<Value, String>;

fn err(e: impl ToString) -> String {
    e.to_string()
}

fn open_url(url: &str) {
    if !url.starts_with("https://") {
        return;
    }
    let _ = if MAC {
        Command::new("open").arg(url).spawn()
    } else {
        store::quiet(Command::new("rundll32").args(["url.dll,FileProtocolHandler", url])).spawn()
    };
}

fn size_text((w, h): (u32, u32)) -> String {
    format!("{w}x{h}")
}

fn parse_size(s: &str) -> Option<(u32, u32)> {
    let (w, h) = s.split_once('x')?;
    Some((w.parse().ok()?, h.parse().ok()?))
}

/// What the host agent does on this PC, on the owner's request.
struct HostHooks {
    app: Arc<App>,
    name: String,
}

impl HostHooks {
    /// Run a config change on a copy so the server restart doesn't happen under the config lock.
    fn reconfigure(&self, f: impl FnOnce(&mut Cfg) -> std::io::Result<bool>) -> bool {
        let mut cfg = self.app.cfg.lock().unwrap().clone();
        let Ok(changed) = f(&mut cfg) else { return false };
        if changed {
            let _ = store::save(&cfg);
            *self.app.cfg.lock().unwrap() = cfg;
            let _ = self.app.server.restart();
        }
        changed
    }
}

impl Hooks for HostHooks {
    fn info(&self) -> Value {
        json!({"name": self.name, "solinear": 1, "os": if MAC { "mac" } else { "windows" }, "screens": screens::list()})
    }
    fn submit(&self, pin: &str, name: &str, ip: &str) -> bool {
        apollo::submit_pin(&self.app.cfg.lock().unwrap().clone(), pin, name, ip)
    }
    fn paired(&self, name: &str) {
        self.app.emit("toast", format!("New device paired: {name}"));
    }
    fn cancel(&self, ip: &str) {
        if MAC {
            apollo::cancel_pending(&self.app.cfg.lock().unwrap().clone(), ip);
        } else if self.app.server.restart().is_ok() {
            // Apollo keys pairing sessions by uniqueid, which every Moonlight shares: a half-done one fails all later tries
            moonlight::wait_for_server("127.0.0.1", Duration::from_secs(20));
        }
    }
    fn action(&self, name: &str) -> bool {
        apollo::run_action(name)
    }
    // shortcut: a restart drops any other stream running on this host at that moment (rare for one owner)
    fn keys(&self, client_os: &str) -> bool {
        // called before every stream, so a device paired since the last one gets full permissions before it launches
        apollo::grant_all_perms(&self.app.cfg.lock().unwrap().clone());
        let mut cfg = self.app.cfg.lock().unwrap().clone();
        let Ok(true) = apollo::set_keys(&apollo::home(), &mut cfg, client_os, MAC) else { return false };
        let _ = store::save(&cfg);
        *self.app.cfg.lock().unwrap() = cfg.clone();
        if apollo::apply_keys_live(&cfg) {
            return false; // patched Apollo took it live: a stream already running keeps going
        }
        let _ = self.app.server.restart(); // older Apollo reads the key map only at startup
        true
    }
    fn screen(&self, id: &str) -> bool {
        self.reconfigure(|c| apollo::set_screen(&apollo::home(), c, id, MAC))
    }
}

impl App {
    pub fn new(net: Arc<dyn Net>) -> Arc<App> {
        Arc::new(App {
            cfg: Mutex::new(store::load()),
            net,
            server: Arc::default(),
            agent: Mutex::new(None),
            session: Mutex::new(None),
            overlay: Mutex::new(None),
            ctl: OnceLock::new(),
            next_id: AtomicU64::new(1),
            handle: OnceLock::new(),
        })
    }

    fn emit(&self, event: &str, payload: impl serde::Serialize + Clone) {
        if let Some(h) = self.handle.get() {
            let _ = h.emit(event, payload);
        }
    }

    pub fn shutdown(&self) {
        if let Some(mut s) = self.session.lock().unwrap().take() {
            let _ = s.child.kill();
        }
        if let Some(mut o) = self.overlay.lock().unwrap().take() {
            let _ = o.kill();
        }
        self.server.stop();
        self.net.stop();
    }

    /// Loopback endpoint the toolbar process forwards its commands to (see overlay.rs). Started on first use.
    fn ctl_start(self: &Arc<Self>) -> (u16, String) {
        self.ctl.get_or_init(|| {
            // not in the streaming server's port range (a free port there would be taken from it)
            let server = (0..20).find_map(|_| tiny_http::Server::http("127.0.0.1:0").ok()
                .filter(|s| !(apollo::AGENT_PORT..=58010).contains(&s.server_addr().to_ip().map_or(0, |a| a.port())))).expect("no free port");
            let port = server.server_addr().to_ip().map_or(0, |a| a.port());
            let token = uuid::Uuid::new_v4().simple().to_string();
            let (app, tok) = (self.clone(), token.clone());
            std::thread::spawn(move || {
                for mut req in server.incoming_requests() {
                    let (app, tok) = (app.clone(), tok.clone());
                    std::thread::spawn(move || {
                        let mut body = String::new();
                        let _ = std::io::Read::read_to_string(&mut std::io::Read::take(req.as_reader(), 4096), &mut body);
                        let authed = req.headers().iter().any(|h| h.field.equiv("X-Token") && h.value.as_str() == tok);
                        let (code, v) = if !authed {
                            (403, json!({"error": "bad token"}))
                        } else {
                            let args = serde_json::from_str(&body).unwrap_or(Value::Null);
                            match app.toolbar_call(req.url().trim_start_matches('/'), &args) {
                                Ok(v) => (200, v),
                                Err(e) => (400, json!({"error": e})),
                            }
                        };
                        let h = tiny_http::Header::from_bytes("Content-Type", "application/json").unwrap();
                        let _ = req.respond(tiny_http::Response::from_string(v.to_string()).with_status_code(code).with_header(h));
                    });
                }
            });
            (port, token)
        }).clone()
    }

    fn spawn_overlay(self: &Arc<Self>) {
        let (port, token) = self.ctl_start();
        let Ok(exe) = std::env::current_exe() else { return };
        let mut slot = self.overlay.lock().unwrap();
        if let Some(mut old) = slot.take() {
            let _ = old.kill(); // a new stream: the toolbar must show the new session
        }
        *slot = Command::new(exe).arg("--overlay").env(crate::overlay::ENV, format!("{port}:{token}")).spawn().ok();
    }

    /// The toolbar's commands (CONTRACT.md), by name, as forwarded from the toolbar process.
    pub fn toolbar_call(self: &Arc<Self>, name: &str, a: &Value) -> R {
        let s = |k: &str| a[k].as_str().map(String::from).ok_or(format!("missing {k}"));
        match name {
            "toolbar_info" => self.toolbar_info(),
            "toolbar_combo" => self.toolbar_combo(s("key")?),
            "toolbar_switch_screen" => self.toolbar_switch_screen(s("id")?),
            "toolbar_reconnect" => self.toolbar_reconnect(s("quality").ok(), s("size").ok(), a["fps"].as_u64().map(|n| n as u32),
                                                          a["hdr"].as_bool(), s("display_mode").ok()),
            "toolbar_action" => self.toolbar_action(s("name")?),
            "toolbar_end" => self.toolbar_end(),
            "toolbar_drag_end" => self.toolbar_drag_end(a["x"].as_f64().ok_or("missing x")?, a["y"].as_f64().ok_or("missing y")?),
            "toolbar_pos" => Ok(self.cfg.lock().unwrap().get("toolbar_pos").cloned().unwrap_or(Value::Null)),
            _ => Err("unknown command".into()),
        }
    }

    fn save_cfg(&self, f: impl FnOnce(&mut Cfg)) {
        let mut c = self.cfg.lock().unwrap();
        f(&mut c);
        let _ = store::save(&c);
    }

    // --- hosting ---
    fn start_hosting(self: &Arc<Self>, me: &Status) -> Result<(), String> {
        let mut slot = self.agent.lock().unwrap();
        if slot.is_some() {
            return Ok(());
        }
        let (Some(login), Some(ip)) = (me.login.clone(), me.ip.clone()) else { return Err("Not signed in.".into()) };
        let hooks = HostHooks { app: self.clone(), name: me.name.clone().unwrap_or_default() };
        let secret = uuid::Uuid::new_v4().simple().to_string();
        let bind = self.net.agent_bind(&ip);
        let agent = Arc::new(Agent { owner: login.clone(), whois: self.net.clone(), hooks: Arc::new(hooks), secret: Some(secret.clone()) });
        // raises if the port is taken (another copy is hosting); the server isn't started then
        *slot = Some(agent::serve(&bind, agent).map_err(err)?);
        let mut cfg = self.cfg.lock().unwrap().clone(); // self-heal: rewrite config and login before each start
        let up = apollo::setup(&mut cfg).and_then(|_| self.server.start()).map_err(err)
            .and_then(|_| self.net.host(true, &login, &bind, &secret));
        match up {
            Ok(()) => *self.cfg.lock().unwrap() = cfg,
            Err(e) => {
                *slot = None;
                self.server.stop();
                return Err(format!("Couldn't start hosting: {e}"));
            }
        }
        Ok(())
    }

    pub fn state(self: &Arc<Self>) -> R {
        let st = self.net.status();
        let is_host = self.cfg.lock().unwrap().get("host").and_then(Value::as_bool).unwrap_or(false);
        let can_host = apollo::server_exe().is_some_and(|p| p.exists());
        let mut error = st.as_ref().err().cloned();
        if let Ok(me) = &st {
            if me.signed_in && is_host && can_host {
                error = self.start_hosting(me).err(); // host mode waits until we're signed in
            }
        }
        let me = st.ok();
        let missing: Vec<&str> = if moonlight::exe().exists() { vec![] } else { vec!["Moonlight"] };
        Ok(json!({
            "signed_in": me.as_ref().is_some_and(|m| m.signed_in),
            "account": me.as_ref().and_then(|m| m.login.clone()),
            "device_name": me.as_ref().and_then(|m| m.name.clone()),
            "is_host": is_host, "can_host": can_host, "mac": MAC, "missing": missing, "error": error,
        }))
    }

    pub fn sign_in(self: &Arc<Self>) -> R {
        let url = self.net.login_url()?;
        open_url(&url);
        Ok(json!({"url": url}))
    }

    pub fn switch_account(self: &Arc<Self>) -> R {
        self.net.logout()?;
        *self.agent.lock().unwrap() = None; // the old account's devices must not keep pairing
        self.sign_in()
    }

    pub fn set_hosting(self: &Arc<Self>, enabled: bool) -> R {
        if enabled {
            if !admin::is_admin() {
                self.save_cfg(|c| {
                    c.insert("host".into(), true.into()); // the elevated copy starts hosting by itself
                });
                if admin::relaunch_elevated() {
                    self.shutdown(); // our server must go before the elevated copy starts its own
                    std::process::exit(0);
                } // UAC declined: host without admin
            }
            let me = self.net.status()?;
            let mut cfg = self.cfg.lock().unwrap().clone();
            apollo::setup(&mut cfg).map_err(err)?;
            *self.cfg.lock().unwrap() = cfg;
            self.start_hosting(&me)?;
        } else {
            let _ = self.net.host(false, "", "", "");
            *self.agent.lock().unwrap() = None;
            self.server.stop();
            self.save_cfg(|c| {
                c.insert("host".into(), false.into());
            });
        }
        self.emit("state-changed", ());
        self.state()
    }

    // --- playing ---
    pub fn hosts(self: &Arc<Self>) -> R {
        let peers: Vec<_> = self.net.peers()?.into_iter().filter(|p| p.online).collect();
        // shortcut: matched by computer name, so another PC with the same name would be marked too; rare for one owner
        let me = moonlight::tailnet_name(&moonlight::hostname());
        let me = me.as_str(); // &str: copied into each thread below
        let found: Vec<Value> = std::thread::scope(|s| {
            let jobs: Vec<_> = peers.iter().map(|p| s.spawn(move || {
                let mut info = moonlight::agent_info(&*self.net, &p.ip)?;
                info["screens"] = json!(screens::from_info(&info)); // every screen gets an id the UI can pick
                let (direct, ms) = self.net.ping(&p.ip).unwrap_or((false, None));
                info["ip"] = p.ip.clone().into();
                info["online"] = true.into();
                info["direct"] = direct.into();
                info["ms"] = ms.map(|m| m.round()).into();
                if info["name"].is_null() {
                    info["name"] = p.name.clone().into();
                }
                // this PC's own installed Tailscale (our helper is a separate node, e.g. "ren-1")
                info["this_device"] = (p.name == me).into();
                Some(info)
            })).collect();
            jobs.into_iter().filter_map(|j| j.join().ok().flatten()).collect()
        });
        Ok(Value::Array(found))
    }

    pub fn settings(self: &Arc<Self>) -> R {
        let c = self.cfg.lock().unwrap();
        let s = |k, d: &str| c.get(k).and_then(Value::as_str).unwrap_or(d).to_string();
        let b = |k, d| c.get(k).and_then(Value::as_bool).unwrap_or(d);
        Ok(json!({"quality": s("quality", "auto"), "display_mode": s("display_mode", "borderless"),
                  "reverse_scroll": b("reverse_scroll", false), "match_keys": b("match_keys", true)}))
    }

    pub fn save_settings(self: &Arc<Self>, quality: String, display_mode: String, reverse_scroll: bool, match_keys: bool) -> R {
        self.save_cfg(|c| {
            c.insert("quality".into(), quality.into());
            c.insert("display_mode".into(), display_mode.into());
            c.insert("reverse_scroll".into(), reverse_scroll.into());
            c.insert("match_keys".into(), match_keys.into());
        });
        self.settings()
    }

    fn fail(msg: &str) -> R {
        Ok(json!({"ok": false, "error": msg}))
    }

    pub fn play(self: &Arc<Self>, ip: String, name: String, screen: Option<String>) -> R {
        let net = &*self.net;
        let addr = match net.connect(&ip) {
            Ok(a) => a,
            Err(e) => return Self::fail(&format!("Couldn't reach that PC: {e}")),
        };
        if !moonlight::wait_for_server(&addr, Duration::from_secs(20)) {
            return Self::fail("That PC's streaming server isn't answering. Is Solinear open on it?");
        }
        if !moonlight::is_paired(&addr) {
            match moonlight::pair(net, &ip, &addr) {
                Ok(true) => {}
                Ok(false) => return Self::fail("Couldn't pair with that PC. Is Solinear open on it?"),
                Err(e) => return Self::fail(&e),
            }
        }
        let cfg = self.cfg.lock().unwrap().clone();
        moonlight::match_keys(net, &ip, &addr, cfg.get("match_keys").and_then(Value::as_bool).unwrap_or(true));
        let info = moonlight::agent_info(net, &ip).unwrap_or(Value::Null);
        let screens = screens::from_info(&info);
        let chosen = screen.as_ref().and_then(|id| screens.iter().find(|s| &s.id == id))
            .or_else(|| screens.iter().find(|s| s.primary)).or(screens.first()).cloned();
        // an older host can only switch live, once the stream is up (see below)
        let live = chosen.as_ref().filter(|c| screen.is_some() && screens::live_only(&c.id) && !c.primary)
            .and_then(|c| screens.iter().position(|s| s.id == c.id)).filter(|n| *n < 12);
        if let (Some(_), Some(c)) = (&screen, chosen.as_ref().filter(|c| !screens::live_only(&c.id))) {
            if let Ok((200, v)) = moonlight::agent_post(net, &ip, "/screen", json!({"id": c.id})) {
                if v["restarted"] == true {
                    std::thread::sleep(Duration::from_secs(1));
                    moonlight::wait_for_server(&addr, Duration::from_secs(20));
                }
            }
        }
        let opts = moonlight::default_opts(&cfg, chosen.as_ref());
        let direct = self.net.ping(&ip).map(|p| p.0).unwrap_or(false);
        let mut slot = self.session.lock().unwrap();
        if let Some(mut old) = slot.take() {
            let _ = old.child.kill();
        }
        let started = std::time::SystemTime::now();
        let (child, ctl) = moonlight::stream(&addr, &opts, direct).map_err(err)?;
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        *slot = Some(Session {
            id, ip, addr, name, os: info["os"].as_str().unwrap_or("windows").into(), screens,
            current_screen: chosen.map(|c| c.id).unwrap_or_default(), opts, child, ctl,
        });
        drop(slot);
        if let Some(n) = live {
            // shortcut: position n in the host's list is assumed to be its server's F<n+1> display (true for
            // Windows' usual monitor order); upgrade the host to the new app for exact ids
            let app = self.clone();
            std::thread::spawn(move || {
                let up = match app.session.lock().unwrap().as_ref() {
                    Some(s) if s.id == id => s.ctl.as_ref().map(|c| c.connected()),
                    _ => return,
                };
                if let Some(up) = up {
                    // straight into the stream once it's up: no key presses, no window focus
                    if moonlight::wait_connected(&up, Duration::from_secs(60)) {
                        if let Some(s) = app.session.lock().unwrap().as_mut().filter(|s| s.id == id) {
                            let _ = s.ctl.as_mut().map(|c| c.send(&format!("screen {}", n + 1)));
                        }
                    }
                    return;
                }
                // keys pressed before the video arrives are lost: wait for it (about 5s, longer while pairing)
                if !moonlight::wait_until_streaming(started, Duration::from_secs(60)) {
                    return;
                }
                for _ in 0..5 {
                    std::thread::sleep(Duration::from_secs(1));
                    let pid = match app.session.lock().unwrap().as_ref() {
                        Some(s) if s.id == id => s.child.id(),
                        _ => return,
                    };
                    if moonlight::send_combo(pid, &format!("f{}", n + 1)).is_ok() {
                        return;
                    }
                }
            });
        }
        self.spawn_overlay();
        let app = self.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_secs(1));
            let mut g = app.session.lock().unwrap();
            match g.as_mut() {
                Some(s) if s.id == id => {
                    if s.child.try_wait().ok().flatten().is_some() {
                        *g = None;
                        drop(g);
                        app.net.disconnect();
                        app.emit("stream-ended", ()); // the toolbar window closes itself on this
                        break;
                    }
                }
                _ => break,
            }
        });
        Ok(json!({"ok": true, "error": null}))
    }

    // --- toolbar ---
    fn with_session<T>(&self, f: impl FnOnce(&mut Session) -> Result<T, String>) -> Result<T, String> {
        f(self.session.lock().unwrap().as_mut().ok_or("No stream is running.")?)
    }

    fn info_of(s: &Session) -> Value {
        json!({"name": s.name, "ip": s.ip, "os": s.os, "screens": s.screens, "current_screen": s.current_screen,
               "opts": {"quality": s.opts.quality, "size": size_text(s.opts.size), "fps": s.opts.fps,
                        "hdr": s.opts.hdr, "display_mode": s.opts.display_mode}})
    }

    pub fn toolbar_info(self: &Arc<Self>) -> R {
        self.with_session(|s| Ok(Self::info_of(s)))
    }

    pub fn toolbar_combo(self: &Arc<Self>, key: String) -> R {
        self.with_session(|s| match s.ctl.as_mut() {
            Some(c) => c.send(&moonlight::ctl_command(&key).ok_or("unknown key")?),
            None => moonlight::send_combo(s.child.id(), &key),
        })?;
        Ok(json!({"ok": true}))
    }

    /// Restart the stream with the session's (already updated) options.
    fn respawn(&self, s: &mut Session) -> Result<(), String> {
        let _ = s.child.kill();
        let _ = s.child.wait();
        let direct = self.net.ping(&s.ip).map(|p| p.0).unwrap_or(false);
        (s.child, s.ctl) = moonlight::stream(&s.addr, &s.opts, direct).map_err(err)?;
        Ok(())
    }

    pub fn toolbar_reconnect(self: &Arc<Self>, quality: Option<String>, size: Option<String>, fps: Option<u32>,
                             hdr: Option<bool>, display_mode: Option<String>) -> R {
        let mut g = self.session.lock().unwrap();
        let s = g.as_mut().ok_or("No stream is running.")?;
        let o = &mut s.opts;
        o.quality = quality.unwrap_or(o.quality.clone());
        o.size = size.as_deref().and_then(parse_size).unwrap_or(o.size);
        o.fps = fps.unwrap_or(o.fps);
        o.hdr = hdr.unwrap_or(o.hdr);
        o.display_mode = display_mode.unwrap_or(o.display_mode.clone());
        self.respawn(s)?;
        Ok(Self::info_of(s))
    }

    pub fn toolbar_switch_screen(self: &Arc<Self>, id: String) -> R {
        // held throughout, so the session watcher doesn't see the old stream die and end the session
        let mut g = self.session.lock().unwrap();
        let s = g.as_mut().ok_or("No stream is running.")?;
        let screen = s.screens.iter().find(|x| x.id == id).ok_or("Unknown screen.")?.clone();
        if s.current_screen == id {
            return Ok(Self::info_of(s)); // already showing: never restart the stream for nothing
        }
        // Same size as the screen being streamed: the host's server switches live on Ctrl+Alt+Shift+F<n>, no restart.
        // shortcut: n = position in the host's list, assumed to match its DXGI order (see screens::list)
        let cur = s.screens.iter().find(|x| x.id == s.current_screen);
        let slot = s.screens.iter().position(|x| x.id == id).filter(|n| *n < 12);
        if let (Some(cur), Some(n)) = (cur, slot) {
            // older hosts (no /screen) can only switch live, whatever the size
            if cur.id != id && ((cur.w, cur.h) == (screen.w, screen.h) || screens::live_only(&id)) {
                match s.ctl.as_mut() {
                    Some(c) => c.send(&format!("screen {}", n + 1))?,
                    None => moonlight::send_combo(s.child.id(), &format!("f{}", n + 1))?,
                }
                s.current_screen = id;
                return Ok(Self::info_of(s));
            }
        }
        if screens::live_only(&id) {
            return Err("That PC can switch only its first 12 screens.".into()); // older host: no /screen to fall back to
        }
        let _ = s.child.kill(); // the host's server restarts on a screen change and would drop it anyway
        let (code, v) = moonlight::agent_post(&*self.net, &s.ip, "/screen", json!({"id": id}))?;
        if code != 200 {
            return Err(v["error"].as_str().unwrap_or("The host refused.").into());
        }
        if v["restarted"] == true {
            std::thread::sleep(Duration::from_secs(1));
            moonlight::wait_for_server(&s.addr, Duration::from_secs(20));
        }
        let (cw, ch, _) = quality::screen();
        s.opts.size = quality::fit(screen.w, screen.h, cw, ch);
        s.current_screen = id;
        self.respawn(s)?;
        Ok(Self::info_of(s))
    }

    pub fn toolbar_action(self: &Arc<Self>, name: String) -> R {
        let ip = self.with_session(|s| Ok(s.ip.clone()))?;
        let (code, v) = moonlight::agent_post(&*self.net, &ip, "/action", json!({"action": name}))?;
        if code != 200 {
            return Err(v["error"].as_str().unwrap_or("The host refused.").into());
        }
        Ok(v)
    }

    pub fn toolbar_end(self: &Arc<Self>) -> R {
        if let Some(mut s) = self.session.lock().unwrap().take() {
            let _ = s.child.kill();
            self.net.disconnect();
        }
        Ok(json!({"ok": true}))
    }

    pub fn toolbar_drag_end(self: &Arc<Self>, x: f64, y: f64) -> R {
        self.save_cfg(|c| {
            c.insert("toolbar_pos".into(), json!({"x": x, "y": y}));
        });
        Ok(json!({"ok": true}))
    }
}

/// `#[tauri::command]` wrappers: each runs the App method on a blocking thread (they wait on processes and HTTP).
macro_rules! commands {
    ($($name:ident($($arg:ident: $ty:ty),*);)*) => {$(
        #[tauri::command(rename_all = "snake_case")]
        pub async fn $name(app: tauri::State<'_, Arc<App>>, $($arg: $ty),*) -> R {
            let app = app.inner().clone();
            tauri::async_runtime::spawn_blocking(move || app.$name($($arg),*)).await.map_err(err)?
        }
    )*};
}

commands! {
    state();
    sign_in();
    switch_account();
    set_hosting(enabled: bool);
    hosts();
    settings();
    save_settings(quality: String, display_mode: String, reverse_scroll: bool, match_keys: bool);
    play(ip: String, name: String, screen: Option<String>);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toolbar_endpoint_needs_its_token_and_reports_no_stream() {
        let app = App::new(Arc::new(crate::net::Stub("no helper".into())));
        let (port, token) = app.ctl_start();
        let post = |tok: &str, path: &str| {
            let a: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).build().into();
            let mut r = a.post(format!("http://127.0.0.1:{port}/{path}")).header("X-Token", tok).send_json(json!({})).unwrap();
            (r.status().as_u16(), r.body_mut().read_json::<Value>().unwrap())
        };
        assert_eq!(post("wrong", "toolbar_info").0, 403);
        assert_eq!(post(&token, "toolbar_info"), (400, json!({"error": "No stream is running."})));
        assert_eq!(post(&token, "nope").0, 400);
        assert!(!(57980..=58010).contains(&port));
    }
}
