//! Picks every stream setting, so users never have to.

pub const RELAY_CAP: u32 = 10_000; // kbps; relayed paths can't carry much more

fn scale(choice: &str) -> f64 {
    match choice {
        "smoother" => 0.7,
        "sharper" => 1.5,
        _ => 1.0,
    }
}

/// Stream at the host screen's shape. Without a virtual display the host can't switch to the client's
/// shape (a 3:2 Mac on a 16:9 PC), so the server pads the picture and absolute mouse positions land off
/// target. Shrink to fit the client screen; never upscale. Even sizes for the encoder.
pub fn fit(host_w: u32, host_h: u32, client_w: u32, client_h: u32) -> (u32, u32) {
    let k = (client_w as f64 / host_w as f64).min(client_h as f64 / host_h as f64).min(1.0);
    ((host_w as f64 * k) as u32 / 2 * 2, (host_h as f64 * k) as u32 / 2 * 2)
}

pub fn bitrate_kbps(w: u32, h: u32, fps: u32, direct: bool, choice: &str) -> u32 {
    let mut kbps = w as f64 * h as f64 * fps as f64 * 0.16 / 1000.0 * scale(choice); // ~20 Mbps at 1080p60
    if !direct {
        kbps = kbps.min(RELAY_CAP as f64);
    }
    kbps.clamp(2_000.0, 150_000.0) as u32
}

pub fn flags(w: u32, h: u32, fps: u32, kbps: u32, display_mode: &str, reverse_scroll: bool, hdr: bool) -> Vec<String> {
    let mode = if ["borderless", "fullscreen", "windowed"].contains(&display_mode) { display_mode } else { "borderless" };
    let v = |s: &str| s.to_string();
    vec![
        v("--resolution"), format!("{w}x{h}"), v("--fps"), fps.to_string(), v("--bitrate"), kbps.to_string(),
        v("--video-codec"), v("auto"), v("--display-mode"), v(mode),
        v(if reverse_scroll { "--reverse-scroll-direction" } else { "--no-reverse-scroll-direction" }),
        v(if hdr { "--hdr" } else { "--no-hdr" }),
        // desktop mouse: the cursor stays free, so the toolbar is reachable. Gaming mode (captured
        // mouse) is toggled live with Ctrl+Alt+Shift+M.
        v("--absolute-mouse"),
        // send Cmd/Win and Alt/Option+Tab to the host while the stream has the keyboard (Gaming mode)
        v("--capture-system-keys"), v("always"),
        v("--game-optimization"), // Apollo only resizes its virtual display with this on
    ]
}

/// Client's real resolution and refresh rate.
#[cfg(windows)]
pub fn screen() -> (u32, u32, u32) {
    use windows::Win32::Graphics::Gdi::*;
    use windows::Win32::UI::WindowsAndMessaging::*;
    unsafe {
        let _ = SetProcessDPIAware(); // otherwise Windows reports scaled-down sizes
        let mut dm = DEVMODEW { dmSize: std::mem::size_of::<DEVMODEW>() as u16, ..Default::default() };
        let hz = if EnumDisplaySettingsW(None, ENUM_CURRENT_SETTINGS, &mut dm).as_bool() { dm.dmDisplayFrequency } else { 0 };
        (GetSystemMetrics(SM_CXSCREEN) as u32, GetSystemMetrics(SM_CYSCREEN) as u32, if hz > 1 { hz } else { 60 })
    }
}

// shortcut: Mac returns a fixed 1920x1080@60 (never upscales, so it only caps the stream); read NSScreen via objc2 later
#[cfg(not(windows))]
pub fn screen() -> (u32, u32, u32) {
    (1920, 1080, 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn about_20mbps_at_1080p60() {
        assert!((bitrate_kbps(1920, 1080, 60, true, "auto") as i64 - 19_906).abs() < 50);
    }

    #[test]
    fn relay_capped() {
        assert_eq!(bitrate_kbps(3840, 2160, 120, false, "auto"), RELAY_CAP);
    }

    #[test]
    fn fit_keeps_host_shape() {
        assert_eq!(fit(1920, 1080, 3024, 1964), (1920, 1080)); // Retina Mac on a 1080p PC
        assert_eq!(fit(3840, 2160, 1920, 1200), (1920, 1080)); // 4K host, smaller laptop
        let (w, h) = fit(2560, 1600, 1366, 768);
        assert!(w <= 1366 && h <= 768 && w % 2 == 0 && h % 2 == 0);
    }

    fn after<'a>(f: &'a [String], k: &str) -> &'a str {
        &f[f.iter().position(|x| x == k).unwrap() + 1]
    }

    #[test]
    fn display_mode_and_scroll_flags() {
        let f = flags(1920, 1080, 60, 20000, "borderless", false, false);
        assert_eq!(after(&f, "--display-mode"), "borderless");
        assert!(f.contains(&"--no-reverse-scroll-direction".into()));
        let f = flags(1920, 1080, 60, 20000, "windowed", true, false);
        assert_eq!(after(&f, "--display-mode"), "windowed");
        assert!(f.contains(&"--reverse-scroll-direction".into()));
        let f = flags(1920, 1080, 60, 20000, "bogus", false, true); // bad saved value falls back
        assert_eq!(after(&f, "--display-mode"), "borderless");
        assert!(f.contains(&"--hdr".into()));
    }

    #[test]
    fn mouse_and_keys_flags_always_present() {
        let f = flags(1920, 1080, 60, 20000, "borderless", false, false);
        assert!(f.contains(&"--absolute-mouse".into()));
        assert_eq!(after(&f, "--capture-system-keys"), "always");
        assert_eq!(after(&f, "--resolution"), "1920x1080");
    }

    #[test]
    fn choice_scales() {
        let auto = bitrate_kbps(1920, 1080, 60, true, "auto");
        assert!(bitrate_kbps(1920, 1080, 60, true, "smoother") < auto);
        assert!(bitrate_kbps(1920, 1080, 60, true, "sharper") > auto);
    }
}
