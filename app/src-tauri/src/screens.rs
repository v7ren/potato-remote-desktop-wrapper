//! This PC's monitors, for the toolbar's screen switcher. `id` is what Apollo calls `output_name`.
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Screen {
    #[serde(default)] // older (Python) hosts send screens without id/name: see from_info
    pub id: String,
    #[serde(default)]
    pub name: String,
    pub w: u32,
    pub h: u32,
    pub primary: bool,
}

/// A host's `/info` screens. Older hosts send no ids: those get "#<position>", which can only be switched to
/// live (Ctrl+Alt+Shift+F<n>), not via `/screen`.
pub fn from_info(info: &serde_json::Value) -> Vec<Screen> {
    let mut list: Vec<Screen> = serde_json::from_value(info["screens"].clone()).unwrap_or_default();
    if list.iter().all(|s| s.id.is_empty()) {
        // older hosts list screens in Windows' own order (main often last); their server numbers displays from the
        // main one. shortcut: assumes main = F1 and the rest keep their order, true for the usual two-screen setup
        list.sort_by_key(|s| !s.primary);
    }
    list.into_iter().enumerate().map(|(i, mut s)| {
        if s.id.is_empty() {
            s.id = format!("#{i}");
        }
        if s.name.is_empty() {
            s.name = format!("Screen {}", i + 1);
        }
        s
    }).collect()
}

/// True for the "#<n>" ids of from_info.
pub fn live_only(id: &str) -> bool {
    id.starts_with('#')
}

#[cfg(windows)]
pub fn list() -> Vec<Screen> {
    use windows::core::BOOL;
    use windows::Win32::Foundation::{LPARAM, RECT};
    use windows::Win32::Graphics::Gdi::*;
    use windows::Win32::UI::WindowsAndMessaging::SetProcessDPIAware;

    unsafe extern "system" fn each(mon: HMONITOR, _: HDC, _: *mut RECT, out: LPARAM) -> BOOL {
        let mut mi = MONITORINFOEXW::default();
        mi.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        if GetMonitorInfoW(mon, &mut mi as *mut _ as *mut MONITORINFO).as_bool() {
            let r = mi.monitorInfo.rcMonitor;
            let dev = String::from_utf16_lossy(&mi.szDevice).trim_end_matches('\0').to_string();
            let n = dev.trim_start_matches(r"\\.\DISPLAY");
            (*(out.0 as *mut Vec<Screen>)).push(Screen {
                name: format!("Display {n}"),
                id: dev,
                w: (r.right - r.left) as u32,
                h: (r.bottom - r.top) as u32,
                primary: mi.monitorInfo.dwFlags & 1 != 0 // MONITORINFOF_PRIMARY,
            });
        }
        true.into()
    }

    let mut out: Vec<Screen> = vec![];
    unsafe {
        let _ = SetProcessDPIAware(); // real pixels, not scaled
        let _ = EnumDisplayMonitors(None, None, Some(each), LPARAM(&mut out as *mut _ as isize));
    }
    // shortcut: DXGI order (what Ctrl+Alt+Shift+F<n> indexes) is assumed to follow the DISPLAYn number
    out.sort_by_key(|s| (s.id.len(), s.id.clone())); // DISPLAY2 before DISPLAY10
    out
}

// shortcut: one screen at the main display's size; list NSScreen displays by CGDirectDisplayID (objc2) later
#[cfg(not(windows))]
pub fn list() -> Vec<Screen> {
    let (w, h, _) = crate::quality::screen();
    vec![Screen { id: "0".into(), name: "Display 1".into(), w, h, primary: true }]
}

#[cfg(test)]
mod tests {
    #[test]
    fn old_hosts_screens_get_position_ids() {
        let info = serde_json::json!({"screens": [{"w": 1920, "h": 1080, "primary": false}, {"id": "x", "name": "Main", "w": 2560, "h": 1440, "primary": true}]});
        let s = super::from_info(&info);
        assert_eq!((s[0].id.as_str(), s[0].name.as_str(), s[0].w), ("#0", "Screen 1", 1920));
        assert_eq!((s[1].id.as_str(), s[1].name.as_str()), ("x", "Main"));
        // an old host (no ids) listing its main screen last: Screen 1 = main = F1
        let old = serde_json::json!({"screens": [{"w": 1920, "h": 1080, "primary": false}, {"w": 2560, "h": 1440, "primary": true}]});
        let s = super::from_info(&old);
        assert_eq!((s[0].id.as_str(), s[0].w, s[0].primary), ("#0", 2560, true));
        assert_eq!((s[1].id.as_str(), s[1].w), ("#1", 1920));
        assert!(super::live_only("#0") && !super::live_only("x"));
        assert!(super::from_info(&serde_json::json!({})).is_empty());
    }

    #[test]
    fn lists_a_primary_screen() {
        let s = super::list();
        println!("{s:?}");
        assert!(s.iter().any(|x| x.primary && x.w > 0));
    }
}
