//! The network layer (Tailscale via the Go helper). Only the interface lives here, so everything else can be
//! tested against a fake.
use crate::agent::Whois;

pub struct Status {
    pub signed_in: bool,
    pub login: Option<String>,
    pub name: Option<String>,
    /// This device's tailnet IP: the only address the agent binds to.
    pub ip: Option<String>,
}

pub struct Peer {
    pub name: String,
    pub ip: String,
}

pub trait Net: Whois {
    fn status(&self) -> Result<Status, String>;
    fn peers(&self) -> Result<Vec<Peer>, String>;
    /// (direct path?, round trip in ms)
    fn ping(&self, ip: &str) -> Result<(bool, Option<f64>), String>;
    /// Starts a login and returns the URL the user must open.
    fn login_url(&self) -> Result<String, String>;
    fn logout(&self) -> Result<(), String>;
}

/// Stand-in until the helper exists: everything fails, nobody is the owner.
pub struct Stub;

const NO_HELPER: &str = "The network helper isn't available yet.";

impl Whois for Stub {
    fn whois(&self, _ip: &str) -> Option<String> {
        None
    }
}

impl Net for Stub {
    fn status(&self) -> Result<Status, String> {
        Err(NO_HELPER.into())
    }
    fn peers(&self) -> Result<Vec<Peer>, String> {
        Err(NO_HELPER.into())
    }
    fn ping(&self, _ip: &str) -> Result<(bool, Option<f64>), String> {
        Err(NO_HELPER.into())
    }
    fn login_url(&self) -> Result<String, String> {
        Err(NO_HELPER.into())
    }
    fn logout(&self) -> Result<(), String> {
        Err(NO_HELPER.into())
    }
}
