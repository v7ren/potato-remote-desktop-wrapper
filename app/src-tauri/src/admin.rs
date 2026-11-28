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

/// Run `program` with `params` elevated (one UAC prompt), without waiting. False if declined or it failed.
#[cfg(windows)]
pub fn run_elevated(program: &str, params: &str) -> bool {
    use windows::core::{w, HSTRING};
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;

    let r = unsafe { ShellExecuteW(None, w!("runas"), &HSTRING::from(program), &HSTRING::from(params), None, SW_HIDE) };
    r.0 as isize > 32
}

#[cfg(not(windows))]
pub fn run_elevated(_program: &str, _params: &str) -> bool {
    false
}
