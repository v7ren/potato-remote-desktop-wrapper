//! Key remaps so the client's own shortcuts work on the other OS. Moonlight sends Windows virtual-key codes
//! (Mac Cmd arrives as Win 0x5B, Option as Alt, Control as Ctrl); the server remaps single keys before
//! injecting them. Pairs of [from, to], keyed by (client OS, host OS).

/// Mac client on a Windows host: Cmd -> Ctrl (Cmd+C = Ctrl+C), Control -> Win. Option is already Alt.
const MAC_ON_WINDOWS: [u32; 8] = [0x5B, 0xA2, 0x5C, 0xA3, 0xA2, 0x5B, 0x11, 0x5B];
/// Windows client on a Mac host: Ctrl -> Cmd (Ctrl+C = Cmd+C, Ctrl+Tab = app switcher), Win -> Control.
const WINDOWS_ON_MAC: [u32; 10] = [0xA2, 0x5B, 0xA3, 0x5C, 0x11, 0x5B, 0x5B, 0xA2, 0x5C, 0xA3];

pub fn keymap(client_os: &str, host_is_mac: bool) -> &'static [u32] {
    match (client_os, host_is_mac) {
        ("mac", false) => &MAC_ON_WINDOWS,
        ("windows", true) => &WINDOWS_ON_MAC,
        _ => &[],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(v: &[u32]) -> std::collections::HashMap<u32, u32> {
        v.chunks(2).map(|p| (p[0], p[1])).collect()
    }

    #[test]
    fn mac_client_on_windows_host() {
        assert_eq!(pairs(keymap("mac", false))[&0x5B], 0xA2); // Cmd -> Ctrl
        assert!(keymap("windows", false).is_empty());
    }

    #[test]
    fn windows_client_on_mac_host() {
        assert_eq!(pairs(keymap("windows", true))[&0xA2], 0x5B); // Ctrl -> Cmd
        assert!(keymap("mac", true).is_empty());
    }
}
