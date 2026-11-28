//! Host agent: the pairing/control HTTP endpoint. Only devices signed in to the owner's account may pair,
//! trigger host actions, or change keyboard/screen. Behind the network helper it listens on loopback only and
//! learns the caller's login from the helper's headers, trusted only with the per-run secret.
use serde_json::{json, Value};
use std::io::Read;
use std::sync::Arc;

/// The caller's Tailscale login, by its IP. None = unknown peer.
pub trait Whois: Send + Sync {
    fn whois(&self, ip: &str) -> Option<String>;
}

/// What the agent does on the host; kept out of here so the validation can be tested alone.
pub trait Hooks: Send + Sync {
    /// `/info` body; its `screens[].id` are the only ids `/screen` accepts.
    fn info(&self) -> Value;
    fn submit(&self, pin: &str, name: &str, client_ip: &str) -> bool;
    fn paired(&self, _name: &str) {}
    fn cancel(&self, client_ip: &str);
    fn action(&self, name: &str) -> bool;
    /// True if the server restarted.
    fn keys(&self, client_os: &str) -> bool;
    /// True if the server restarted.
    fn screen(&self, id: &str) -> bool;
}

pub const ACTIONS: [&str; 7] = ["lock", "signout", "restart", "shutdown", "explorer", "taskmgr", "control"];

pub struct Agent {
    pub owner: String,
    pub whois: Arc<dyn Whois>,
    pub hooks: Arc<dyn Hooks>,
    /// Set when the helper fronts us: then only a request carrying this secret has a known caller.
    pub secret: Option<String>,
}

fn err(code: u16, msg: &str) -> (u16, Value) {
    (code, json!({ "error": msg }))
}

impl Agent {
    /// `login`: the caller's tailnet login, None if unknown.
    pub fn handle(&self, method: &str, path: &str, peer: &str, login: Option<&str>, body: &[u8]) -> (u16, Value) {
        if method == "GET" && path == "/info" {
            return (200, self.hooks.info());
        }
        if method != "POST" || !["/pair", "/cancel", "/action", "/keys", "/screen"].contains(&path) {
            return err(404, "not found");
        }
        if self.owner.is_empty() || !login.is_some_and(|l| l.eq_ignore_ascii_case(&self.owner)) {
            return err(403, "This device is signed in to a different account.");
        }
        if path == "/cancel" {
            self.hooks.cancel(peer); // only ever the caller's own pending request
            return (200, json!({"status": true}));
        }
        if body.len() > 4096 {
            return err(413, "too large");
        }
        let Ok(body) = serde_json::from_slice::<Value>(if body.is_empty() { b"{}" } else { body }) else {
            return err(400, "bad json");
        };
        let s = |k: &str| body[k].as_str().unwrap_or("").to_string();
        match path {
            "/action" => {
                let name = s("action");
                if !ACTIONS.contains(&name.as_str()) {
                    return err(400, "unknown action");
                }
                (200, json!({"status": self.hooks.action(&name)}))
            }
            "/keys" => {
                let os = s("client_os");
                if !["", "mac", "windows"].contains(&os.as_str()) {
                    return err(400, "unknown os");
                }
                (200, json!({"status": true, "restarted": self.hooks.keys(&os)}))
            }
            "/screen" => {
                let id = s("id");
                let known = self.hooks.info()["screens"].as_array()
                    .is_some_and(|a| a.iter().any(|x| x["id"].as_str() == Some(id.as_str())));
                if !known {
                    return err(400, "unknown screen");
                }
                (200, json!({"status": true, "restarted": self.hooks.screen(&id)}))
            }
            _ => {
                let pin = s("pin");
                let name: String = body["name"].as_str().unwrap_or("device").chars().take(64).collect();
                if pin.len() != 4 || !pin.bytes().all(|b| b.is_ascii_digit()) {
                    return err(400, "bad pin");
                }
                let ok = self.hooks.submit(&pin, &name, peer);
                if ok {
                    self.hooks.paired(&name);
                }
                (200, json!({"status": ok}))
            }
        }
    }
}

pub struct Running {
    server: Arc<tiny_http::Server>,
    #[allow(dead_code)] // tests bind port 0 and read the real one back
    pub port: u16,
}

impl Drop for Running {
    fn drop(&mut self) {
        self.server.unblock(); // ends the accept loop, which then releases the port
    }
}

/// Bind `addr` ("ip:port"; port 0 picks a free one). Fails if the port is taken, so only one copy ever
/// hosts (Windows doesn't let a second listener share the port).
pub fn serve(addr: &str, agent: Arc<Agent>) -> std::io::Result<Running> {
    let server = Arc::new(tiny_http::Server::http(addr).map_err(std::io::Error::other)?);
    let port = server.server_addr().to_ip().map_or(0, |a| a.port());
    let s = server.clone();
    std::thread::spawn(move || {
        for mut req in s.incoming_requests() {
            let agent = agent.clone();
            std::thread::spawn(move || {
                let mut body = Vec::new();
                let _ = req.as_reader().take(4097).read_to_end(&mut body);
                let peer = req.remote_addr().map(|a| a.ip().to_string()).unwrap_or_default();
                let header = |n: &str| req.headers().iter().find(|h| h.field.as_str().as_str().eq_ignore_ascii_case(n)).map(|h| h.value.as_str().to_string());
                let login = match &agent.secret {
                    // never trust loopback itself: any local program can connect here
                    Some(s) => (header("X-Solinear-Secret").as_deref() == Some(s.as_str()))
                        .then(|| header("X-Solinear-Login")).flatten().filter(|l| !l.is_empty()),
                    None => agent.whois.whois(&peer),
                };
                let (code, v) = agent.handle(req.method().as_str(), req.url(), &peer, login.as_deref(), &body);
                let h = tiny_http::Header::from_bytes("Content-Type", "application/json").unwrap();
                let _ = req.respond(tiny_http::Response::from_string(v.to_string()).with_status_code(code).with_header(h));
            });
        }
    });
    Ok(Running { server, port })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct Fake {
        login: Mutex<Option<String>>,
        log: Mutex<Vec<String>>,
    }

    impl Whois for Fake {
        fn whois(&self, _ip: &str) -> Option<String> {
            self.login.lock().unwrap().clone()
        }
    }

    impl Hooks for Fake {
        fn info(&self) -> Value {
            json!({"name": "GAMER", "screens": [{"id": r"\\.\DISPLAY1"}, {"id": r"\\.\DISPLAY2"}]})
        }
        fn submit(&self, pin: &str, name: &str, _ip: &str) -> bool {
            self.log.lock().unwrap().push(format!("pin:{pin}:{name}"));
            true
        }
        fn cancel(&self, _ip: &str) {}
        fn action(&self, name: &str) -> bool {
            self.log.lock().unwrap().push(name.into());
            true
        }
        fn keys(&self, os: &str) -> bool {
            self.log.lock().unwrap().push(format!("keys:{os}"));
            true
        }
        fn screen(&self, id: &str) -> bool {
            self.log.lock().unwrap().push(format!("screen:{id}"));
            true
        }
    }

    struct T {
        fake: Arc<Fake>,
        run: Running,
    }

    impl T {
        fn new() -> T {
            let fake = Arc::new(Fake { login: Mutex::new(Some("owner@gmail.com".into())), log: Mutex::new(vec![]) });
            let agent = Arc::new(Agent { owner: "owner@gmail.com".into(), whois: fake.clone(), hooks: fake.clone(), secret: None });
            T { fake, run: serve("127.0.0.1:0", agent).unwrap() }
        }

        fn post(&self, path: &str, body: Value) -> (u16, Value) {
            let a: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).build().into();
            let mut r = a.post(format!("http://127.0.0.1:{}{path}", self.run.port)).send_json(body).unwrap();
            (r.status().as_u16(), r.body_mut().read_json().unwrap())
        }

        fn who(&self, login: Option<&str>) {
            *self.fake.login.lock().unwrap() = login.map(Into::into);
        }

        fn log(&self) -> Vec<String> {
            self.fake.log.lock().unwrap().clone()
        }
    }

    #[test]
    fn second_agent_on_same_port_refused() {
        let t = T::new();
        let fake = t.fake.clone();
        let agent = Arc::new(Agent { owner: "o".into(), whois: fake.clone(), hooks: fake, secret: None });
        assert!(serve(&format!("127.0.0.1:{}", t.run.port), agent).is_err());
    }

    #[test]
    fn info() {
        let t = T::new();
        let mut r = ureq::get(format!("http://127.0.0.1:{}/info", t.run.port)).call().unwrap();
        assert_eq!(r.body_mut().read_json::<Value>().unwrap()["name"], "GAMER");
    }

    #[test]
    fn owner_can_pair() {
        let t = T::new();
        assert_eq!(t.post("/pair", json!({"pin": "0420", "name": "laptop"})), (200, json!({"status": true})));
        assert_eq!(t.log(), ["pin:0420:laptop"]);
    }

    #[test]
    fn other_account_and_unknown_peer_refused() {
        let t = T::new();
        for who in [Some("stranger@gmail.com"), None] {
            t.who(who);
            assert_eq!(t.post("/pair", json!({"pin": "0420"})).0, 403);
            assert_eq!(t.post("/cancel", json!({})).0, 403);
        }
        assert!(t.log().is_empty());
    }

    #[test]
    fn cancel_for_owner() {
        assert_eq!(T::new().post("/cancel", json!({})), (200, json!({"status": true})));
    }

    #[test]
    fn action_owner_only_and_known_names_only() {
        let t = T::new();
        assert_eq!(t.post("/action", json!({"action": "lock"})), (200, json!({"status": true})));
        assert_eq!(t.post("/action", json!({"action": "format c:"})).0, 400);
        t.who(Some("stranger@gmail.com"));
        assert_eq!(t.post("/action", json!({"action": "shutdown"})).0, 403);
        assert_eq!(t.log(), ["lock"]);
    }

    #[test]
    fn keys_owner_only_and_known_os_only() {
        let t = T::new();
        assert_eq!(t.post("/keys", json!({"client_os": "mac"})), (200, json!({"status": true, "restarted": true})));
        assert_eq!(t.post("/keys", json!({"client_os": "linux; rm -rf"})).0, 400);
        t.who(Some("stranger@gmail.com"));
        assert_eq!(t.post("/keys", json!({"client_os": "windows"})).0, 403);
        assert_eq!(t.log(), ["keys:mac"]);
    }

    #[test]
    fn screen_owner_only_and_known_ids_only() {
        let t = T::new();
        assert_eq!(t.post("/screen", json!({"id": r"\\.\DISPLAY2"})), (200, json!({"status": true, "restarted": true})));
        assert_eq!(t.post("/screen", json!({"id": r"\\.\DISPLAY9"})).0, 400);
        assert_eq!(t.post("/screen", json!({})).0, 400);
        t.who(Some("stranger@gmail.com"));
        assert_eq!(t.post("/screen", json!({"id": r"\\.\DISPLAY1"})).0, 403);
        assert_eq!(t.log(), [r"screen:\\.\DISPLAY2"]);
    }

    #[test]
    fn bad_pin_rejected() {
        let t = T::new();
        for pin in ["12", "abcd", "12345", ""] {
            assert_eq!(t.post("/pair", json!({"pin": pin})).0, 400);
        }
        assert!(t.log().is_empty());
    }

    #[test]
    fn unknown_path_404() {
        assert_eq!(T::new().post("/nope", json!({})).0, 404);
    }

    #[test]
    fn behind_helper_only_the_secret_makes_a_caller_known() {
        let fake = Arc::new(Fake { login: Mutex::new(Some("owner@gmail.com".into())), log: Mutex::new(vec![]) });
        let agent = Arc::new(Agent { owner: "owner@gmail.com".into(), whois: fake.clone(), hooks: fake.clone(), secret: Some("s3cret".into()) });
        let run = serve("127.0.0.1:0", agent).unwrap();
        let post = |hs: &[(&str, &str)]| {
            let a: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).build().into();
            let mut r = a.post(format!("http://127.0.0.1:{}/action", run.port));
            for (k, v) in hs {
                r = r.header(*k, *v);
            }
            r.send_json(json!({"action": "lock"})).unwrap().status().as_u16()
        };
        // whois would say "owner", but loopback is never trusted on its own
        assert_eq!(post(&[]), 403);
        assert_eq!(post(&[("X-Solinear-Login", "owner@gmail.com")]), 403);
        assert_eq!(post(&[("X-Solinear-Login", "owner@gmail.com"), ("X-Solinear-Secret", "wrong")]), 403);
        assert_eq!(post(&[("X-Solinear-Login", "other@gmail.com"), ("X-Solinear-Secret", "s3cret")]), 403);
        assert_eq!(post(&[("X-Solinear-Login", ""), ("X-Solinear-Secret", "s3cret")]), 403);
        assert_eq!(post(&[("X-Solinear-Login", "Owner@Gmail.com"), ("X-Solinear-Secret", "s3cret")]), 200);
        assert_eq!(fake.log.lock().unwrap().clone(), vec!["lock".to_string()]);
    }
}
