//! This PC's monitors, for the toolbar's screen switcher. `id` is what Apollo calls `output_name`.
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Screen {
    pub id: String,
    pub name: String,
    pub w: u32,
    pub h: u32,
    pub primary: bool,
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
    fn lists_a_primary_screen() {
        let s = super::list();
        println!("{s:?}");
        assert!(s.iter().any(|x| x.primary && x.w > 0));
    }
}
