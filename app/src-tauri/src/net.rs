//! The network layer: Solinear's own Tailscale node, run by the Go helper `solinear-net` (see CONTRACT.md).
//! Everything else talks to it through `Net`, so it can be faked in tests.
use crate::agent::Whois;
use crate::apollo::AGENT_PORT;
use crate::store;
use serde_json::{json, Value};
use std::io::Write;
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub struct Status {
    pub signed_in: bool,
    pub login: Option<String>,
    pub name: Option<String>,
    /// This device's tailnet IP.
    pub ip: Option<String>,
}

pub struct Peer {
    pub name: String,
    pub ip: String,
    pub online: bool,
}

pub trait Net: Whois {
    fn status(&self) -> Result<Status, String>;
    fn peers(&self) -> Result<Vec<Peer>, String>;
    /// (direct path?, round trip in ms)
    fn ping(&self, ip: &str) -> Result<(bool, Option<f64>), String>;
    /// Starts a login and returns the URL the user must open.
    fn login_url(&self) -> Result<String, String>;
    fn logout(&self) -> Result<(), String>;
    /// Call a host's agent: (status code, body). Err = unreachable.
    fn agent(&self, method: &str, ip: &str, path: &str, body: Option<Value>, timeout: Duration) -> Result<(u16, Value), String>;
    /// Where our own agent listens.
    fn agent_bind(&self, my_ip: &str) -> String;
    /// Expose (or stop exposing) this PC's agent and streaming ports to the owner's devices.
    fn host(&self, enabled: bool, owner: &str, agent: &str, secret: &str) -> Result<(), String>;
    /// Make `ip`'s streaming server reachable for Moonlight; returns the address to give Moonlight.
    fn connect(&self, ip: &str) -> Result<String, String>;
    fn disconnect(&self) {}
    fn stop(&self) {}
}

/// Plain HTTP to a host's agent on its tailnet IP (the Stub's path, and tests).
pub fn direct_agent(method: &str, ip: &str, path: &str, body: Option<Value>, timeout: Duration) -> Result<(u16, Value), String> {
    let a: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).timeout_global(Some(timeout)).build().into();
    let url = format!("http://{ip}:{AGENT_PORT}{path}");
    let mut r = match body {
        Some(b) if method == "POST" => a.post(url).send_json(b),
        _ => a.get(url).call(),
    }.map_err(|e| e.to_string())?;
    Ok((r.status().as_u16(), r.body_mut().read_json().unwrap_or(Value::Null)))
}

/// Stand-in when the helper can't start: nothing works, nobody is the owner.
pub struct Stub(pub String);

impl Whois for Stub {
    fn whois(&self, _ip: &str) -> Option<String> {
        None
    }
}

impl Net for Stub {
    fn status(&self) -> Result<Status, String> {
        Err(self.0.clone())
    }
    fn peers(&self) -> Result<Vec<Peer>, String> {
        Err(self.0.clone())
    }
    fn ping(&self, _ip: &str) -> Result<(bool, Option<f64>), String> {
        Err(self.0.clone())
    }
    fn login_url(&self) -> Result<String, String> {
        Err(self.0.clone())
    }
    fn logout(&self) -> Result<(), String> {
        Err(self.0.clone())
    }
    fn agent(&self, method: &str, ip: &str, path: &str, body: Option<Value>, timeout: Duration) -> Result<(u16, Value), String> {
        direct_agent(method, ip, path, body, timeout)
    }
    fn agent_bind(&self, my_ip: &str) -> String {
        format!("{my_ip}:{AGENT_PORT}")
    }
    fn host(&self, _: bool, _: &str, _: &str, _: &str) -> Result<(), String> {
        Ok(())
    }
    fn connect(&self, ip: &str) -> Result<String, String> {
        Ok(ip.into())
    }
}

/// The running `solinear-net` process and its local control API.
pub struct Helper {
    port: u16,
    token: String,
    proc: Mutex<Option<(Child, ChildStdin)>>,
}

fn helper_exe() -> Option<PathBuf> {
    let name = if store::MAC { "solinear-net" } else { "solinear-net.exe" };
    let exe = std::env::current_exe().ok()?;
    // installed: next to the app; dev: app/helper/ above target/debug
    exe.ancestors().skip(1).flat_map(|d| [d.join(name), d.join("helper").join(name)]).find(|p| p.exists())
}

impl Helper {
    pub fn start(login_server: Option<&str>) -> Result<Helper, String> {
        let exe = helper_exe().ok_or("The network helper (solinear-net) is missing. Reinstall Solinear.")?;
        let dir = store::data_dir();
        std::fs::create_dir_all(dir.join("net")).map_err(|e| e.to_string())?;
        // shortcut: the port is free when picked but could be taken before the helper binds it (then status() errors)
        let port = std::net::TcpListener::bind("127.0.0.1:0").and_then(|l| l.local_addr()).map_err(|e| e.to_string())?.port();
        let token = uuid::Uuid::new_v4().simple().to_string();
        let log = std::fs::File::create(dir.join("net.log")).map_err(|e| e.to_string())?; // tsnet is chatty; never an undrained pipe
        let host = std::env::var("COMPUTERNAME").or_else(|_| std::env::var("HOSTNAME")).unwrap_or_else(|_| "solinear".into());
        let mut c = Command::new(exe);
        c.args(["--state", &dir.join("net").to_string_lossy(), "--token", &token, "--hostname", &host,
                "--control", &format!("127.0.0.1:{port}"), "--port", "0"]); // 0: never clash with another tailscaled
        if let Some(url) = login_server.filter(|u| u.starts_with("https://")) {
            c.args(["--login-server", url]);
        }
        let mut child = store::quiet(&mut c).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(log).spawn().map_err(|e| e.to_string())?;
        let stdin = child.stdin.take().expect("piped"); // the helper exits when this closes, i.e. when we die
        Ok(Helper { port, token, proc: Mutex::new(Some((child, stdin))) })
    }

    fn call(&self, method: &str, path: &str, body: Option<Value>, timeout: Duration) -> Result<(u16, Value), String> {
        let a: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).timeout_global(Some(timeout)).build().into();
        let url = format!("http://127.0.0.1:{}{path}", self.port);
        let auth = format!("Bearer {}", self.token);
        let end = Instant::now() + Duration::from_secs(5); // just started: the control API takes a moment to listen
        loop {
            let r = if method == "POST" {
                a.post(&url).header("Authorization", &auth).send_json(body.clone().unwrap_or(json!({})))
            } else {
                a.get(&url).header("Authorization", &auth).call()
            };
            match r {
                Ok(mut r) => return Ok((r.status().as_u16(), r.body_mut().read_json().unwrap_or(Value::Null))),
                Err(ureq::Error::ConnectionFailed | ureq::Error::Io(_)) if Instant::now() < end => std::thread::sleep(Duration::from_millis(200)),
                Err(e) => return Err(format!("Network helper not answering: {e}")),
            }
        }
    }

    fn ok(&self, method: &str, path: &str, body: Option<Value>) -> Result<Value, String> {
        match self.call(method, path, body, Duration::from_secs(20))? {
            (200, v) => Ok(v),
            (_, v) => Err(v["error"].as_str().unwrap_or("network helper error").into()),
        }
    }
}

impl Whois for Helper {
    fn whois(&self, ip: &str) -> Option<String> {
        self.ok("GET", &format!("/whois?ip={ip}"), None).ok()?["login"].as_str().map(String::from)
    }
}

impl Net for Helper {
    fn status(&self) -> Result<Status, String> {
        let end = Instant::now() + Duration::from_secs(8); // with a saved login it goes Starting -> Running by itself
        let v = loop {
            let v = self.ok("GET", "/status", None)?;
            if !matches!(v["state"].as_str(), Some("NoState" | "Starting")) || Instant::now() > end {
                break v;
            }
            std::thread::sleep(Duration::from_millis(300));
        };
        let me = &v["self"];
        let s = |k: &str| me[k].as_str().map(String::from);
        Ok(Status { signed_in: v["state"] == "Running" && me.is_object(), login: s("login"), name: s("name"), ip: s("ip") })
    }
    fn peers(&self) -> Result<Vec<Peer>, String> {
        let v = self.ok("GET", "/status", None)?;
        Ok(v["peers"].as_array().into_iter().flatten().filter_map(|p| Some(Peer {
            name: p["name"].as_str()?.into(), ip: p["ip"].as_str()?.into(), online: p["online"] == true,
        })).collect())
    }
    fn ping(&self, ip: &str) -> Result<(bool, Option<f64>), String> {
        let v = self.ok("GET", &format!("/ping?ip={ip}"), None)?;
        Ok((v["direct"] == true, v["ms"].as_f64()))
    }
    fn login_url(&self) -> Result<String, String> {
        let v = self.ok("POST", "/login", Some(json!({})))?;
        v["auth_url"].as_str().map(String::from).ok_or("Already signed in.".into())
    }
    fn logout(&self) -> Result<(), String> {
        self.ok("POST", "/logout", Some(json!({}))).map(|_| ())
    }
    fn agent(&self, method: &str, ip: &str, path: &str, body: Option<Value>, timeout: Duration) -> Result<(u16, Value), String> {
        let (code, v) = self.call(method, &format!("/peer/{ip}/agent{path}"), body, timeout)?;
        if code == 502 {
            return Err(v["error"].as_str().unwrap_or("host unreachable").into());
        }
        Ok((code, v))
    }
    fn agent_bind(&self, _my_ip: &str) -> String {
        format!("127.0.0.1:{AGENT_PORT}") // only the helper's tailnet proxy reaches it
    }
    fn host(&self, enabled: bool, owner: &str, agent: &str, secret: &str) -> Result<(), String> {
        self.ok("POST", "/host", Some(json!({"enabled": enabled, "owner": owner, "agent": agent, "secret": secret}))).map(|_| ())
    }
    fn connect(&self, ip: &str) -> Result<String, String> {
        // shortcut: every host is "::1" to Moonlight, which tells hosts apart by their server id; fine for a few hosts
        self.ok("POST", "/connect", Some(json!({"ip": ip}))).map(|_| "::1".into())
    }
    fn disconnect(&self) {
        let _ = self.ok("POST", "/disconnect", Some(json!({})));
    }
    fn stop(&self) {
        if let Some((mut child, mut stdin)) = self.proc.lock().unwrap().take() {
            let _ = stdin.flush();
            drop(stdin);
            let _ = child.kill();
            let _ = child.wait(); // its UDP port and state dir must be free before an elevated copy starts its own
        }
    }
}
