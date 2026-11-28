//! Paths and the local config file. Nothing here ever leaves this PC.
use serde_json::{Map, Value};
use std::path::PathBuf;

pub type Cfg = Map<String, Value>;
pub const MAC: bool = cfg!(target_os = "macos");

pub fn data_dir() -> PathBuf {
    if MAC {
        PathBuf::from(std::env::var("HOME").unwrap_or_default()).join("Library/Application Support/Solinear")
    } else {
        PathBuf::from(std::env::var("APPDATA").unwrap_or_default()).join("Solinear")
    }
}

/// Bundled tailscale/, apollo/, moonlight/. Installed: next to the exe. Dev: the repo's bin\ some levels up.
pub fn bin() -> PathBuf {
    let exe = std::env::current_exe().unwrap_or_default();
    exe.ancestors().skip(1).map(|d| d.join("bin")).find(|b| b.join("apollo").exists() || b.join("moonlight").exists())
        .unwrap_or_else(|| exe.parent().unwrap_or(&exe).join("bin"))
}

pub fn load() -> Cfg {
    std::fs::read_to_string(data_dir().join("config.json")).ok()
        .and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

pub fn save(cfg: &Cfg) -> std::io::Result<()> {
    let dir = data_dir();
    std::fs::create_dir_all(&dir)?;
    let tmp = dir.join("config.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(cfg)?)?;
    std::fs::rename(tmp, dir.join("config.json")) // atomic, so a crash never leaves a half-written config
}

/// Run a child without flashing a console window.
pub fn quiet(cmd: &mut std::process::Command) -> &mut std::process::Command {
    #[cfg(windows)]
    std::os::windows::process::CommandExt::creation_flags(cmd, 0x0800_0000); // CREATE_NO_WINDOW
    cmd
}
