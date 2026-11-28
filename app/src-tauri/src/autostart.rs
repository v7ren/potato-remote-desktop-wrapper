//! Start Solinear when the user logs in.
//! Windows: a scheduled task at logon with the highest privileges, because hosting needs admin (input into games
//! that run as admin). Creating it needs admin too: done directly when we already are, else via one UAC prompt.
//! macOS: a LaunchAgent in ~/Library/LaunchAgents.
use crate::store::{self, MAC};
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

const TASK: &str = "Solinear";
const AGENT: &str = "app.solinear";

fn exe() -> Result<PathBuf, String> {
    std::env::current_exe().map_err(|e| e.to_string())
}

fn plist() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(format!("Library/LaunchAgents/{AGENT}.plist"))
}

/// schtasks arguments to add (or remove) the logon task for `exe`.
pub fn task_args(on: bool, exe: &str) -> Vec<String> {
    // schtasks runs /TR as a command line: a path with spaces needs quotes. --background: straight to the tray
    let tr = format!("\"{exe}\" --background");
    let v = if on {
        vec!["/Create", "/TN", TASK, "/TR", &tr, "/SC", "ONLOGON", "/RL", "HIGHEST", "/F"]
    } else {
        vec!["/Delete", "/TN", TASK, "/F"]
    };
    v.into_iter().map(String::from).collect()
}

pub fn launch_agent(exe: &str) -> String {
    let esc = exe.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    format!(r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>Label</key><string>{AGENT}</string>
  <key>ProgramArguments</key><array><string>{esc}</string><string>--background</string></array>
  <key>RunAtLoad</key><true/>
</dict></plist>
"#)
}

pub fn enabled() -> bool {
    if MAC {
        return plist().exists();
    }
    store::quiet(Command::new("schtasks").args(["/Query", "/TN", TASK]))
        .output().map(|o| o.status.success()).unwrap_or(false)
}

pub fn set(on: bool) -> Result<(), String> {
    let exe = exe()?;
    if MAC {
        let p = plist();
        return if on {
            std::fs::create_dir_all(p.parent().unwrap_or(&p)).and_then(|_| std::fs::write(&p, launch_agent(&exe.to_string_lossy())))
                .map_err(|e| e.to_string())
        } else {
            match std::fs::remove_file(&p) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
                _ => Ok(()),
            }
        };
    }
    let args = task_args(on, &exe.to_string_lossy());
    if crate::admin::is_admin() {
        let out = store::quiet(Command::new("schtasks").args(&args)).output().map_err(|e| e.to_string())?;
        if !out.status.success() {
            return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
        }
        return Ok(());
    }
    // not admin: one UAC prompt for schtasks itself, then wait for the change to show
    let line = args.iter().map(|a| if a.contains(' ') || a.contains('"') { format!("\"{}\"", a.replace('"', "\\\"")) } else { a.clone() })
        .collect::<Vec<_>>().join(" ");
    if !crate::admin::run_elevated("schtasks.exe", &line) {
        return Err("Windows didn't allow it (the admin prompt was declined).".into());
    }
    let end = Instant::now() + Duration::from_secs(20);
    while Instant::now() < end {
        if enabled() == on {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    Err("Starting at login didn't change.".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_args_quote_the_path_and_ask_for_admin() {
        let a = task_args(true, r"C:\Program Files\Solinear\solinear.exe");
        assert_eq!(a[..4], ["/Create", "/TN", "Solinear", "/TR"]);
        assert_eq!(a[4], r#""C:\Program Files\Solinear\solinear.exe" --background"#);
        assert!(a.windows(2).any(|w| w == ["/RL", "HIGHEST"]) && a.windows(2).any(|w| w == ["/SC", "ONLOGON"]));
        assert_eq!(task_args(false, "x"), ["/Delete", "/TN", "Solinear", "/F"]);
    }

    #[test]
    fn launch_agent_runs_the_app_at_load() {
        let p = launch_agent("/Applications/Solinear.app/Contents/MacOS/solinear");
        assert!(p.contains("<string>/Applications/Solinear.app/Contents/MacOS/solinear</string>"));
        assert!(p.contains("<key>RunAtLoad</key><true/>") && p.contains("<string>app.solinear</string>"));
    }
}
