//! Windows: hosting needs admin, because Windows drops input that a non-admin Apollo sends to admin windows
//! (games with anti-cheat). Port of Python's ensure_admin.

#[cfg(windows)]
pub fn is_admin() -> bool {
    unsafe { windows::Win32::UI::Shell::IsUserAnAdmin().as_bool() }
}

#[cfg(not(windows))]
pub fn is_admin() -> bool {
    true
}

/// Start another copy of this exe elevated, with the same arguments. False if the user declined the UAC prompt
/// (or it failed): keep running as we are. The caller must clean up its children and exit on true.
#[cfg(windows)]
pub fn relaunch_elevated() -> bool {
    use windows::core::{w, HSTRING};
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let Ok(exe) = std::env::current_exe() else { return false };
    let args: Vec<String> = std::env::args().skip(1).map(|a| format!("\"{a}\"")).collect();
    let r = unsafe {
        ShellExecuteW(None, w!("runas"), &HSTRING::from(exe.as_os_str()), &HSTRING::from(args.join(" ")), None, SW_SHOWNORMAL)
    };
    r.0 as isize > 32
}

#[cfg(not(windows))]
pub fn relaunch_elevated() -> bool {
    false
}
