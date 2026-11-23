# Solinear — Plan

Open-source, zero-server remote game streaming. Free alternative to Parsec.
Packages **Apollo** (Sunshine fork, host), **Moonlight** (client) and **Tailscale** (mesh), with one sign-in per device.

## Principles

- **Zero server.** Nothing Solinear-owned runs anywhere. Only external dependency: Tailscale's free coordination server (or the user's own Headscale).
- **One human action per device:** install, then sign in. No IPs, ports, PINs, tokens or pasted keys.
- **No settings.** Solinear picks everything. The user gets one choice: Quality (Auto / Smoother / Sharper).
- **Tailscale and Moonlight are invisible, not hidden.** Users never see their windows or setup screens. Credit both in About and in the license notices.

## Core idea: network membership is the identity check

Every device signs in to the same Tailscale account (Google / GitHub / Microsoft / Apple).
Once a device is on the private network it can reach the host directly, and the host checks the asking device's login with `tailscale whois <peer-ip>`.
Same login as the owner → pair. Different login → refuse.

## User flow

### Host (gaming PC)
1. `[ Set up this PC ]`
2. Silent install: Apollo + virtual display, plus Tailscale as a background service.
3. `[ Sign in ]` → system browser → Tailscale's provider picker → Google etc.
4. "✓ REN-DESKTOP is ready. Install Solinear on another device and sign in with the same account."

### Client (laptop, etc.)
1. `[ Sign in ]` → same account.
2. Host list appears automatically → `[ ▶ Play ]` → straight into the fullscreen stream.

## v1 stack: a lightweight Python app

One Python program that only runs and coordinates other programs: it starts Apollo, Moonlight and Tailscale and passes data between them.

| Need | Use | Why |
|---|---|---|
| UI | `pywebview` (uses Edge WebView2, already in Windows 11) | One dependency, HTML/CSS UI that looks modern. Zero-dependency fallback: `tkinter` |
| Run CLIs | `subprocess` | stdlib |
| Host agent | `http.server` (`ThreadingHTTPServer`) | stdlib |
| Call Apollo API | `urllib.request` + `ssl` (Apollo uses a self-signed cert on localhost) | stdlib |
| Config | `json` file in `%APPDATA%\Solinear` | stdlib |
| Ship | PyInstaller single `.exe` + an Inno Setup installer that bundles Apollo, moonlight-qt and tailscaled | Installer needs admin rights once, for Apollo's driver and the Tailscale service |

Only one third-party dependency: `pywebview`.

### Layout

```
solinear/
  main.py        # starts the UI, chooses host or client mode
  ui/index.html  # the only screens users see
  tailnet.py     # tailscale login / status / whois / ping (wraps the CLI's --json output)
  host.py        # Apollo setup + the host agent (/info, /pair)
  client.py      # find hosts, pair, run `moonlight stream`
  quality.py     # picks resolution / fps / codec / bitrate
```

shortcut: plain CLI wrappers, not Tailscale's LocalAPI. Switch to the LocalAPI if parsing the CLI output proves fragile.

## How Tailscale stays out of sight

| What users would normally see | What Solinear does instead |
|---|---|
| Tailscale installer + tray app | Ship only `tailscaled.exe` + `tailscale.exe` inside Solinear's folder; no GUI, no tray icon |
| `tailscale up` / login screens | Run `tailscale login` ourselves, read the auth URL, open it in the system browser behind our own "Sign in" button |
| Tailscale's admin console | Never needed: no auth keys, no ACL edits (pairing is gated by `whois`) |
| Device names like `desktop-8f3k2` | `--hostname=<friendly PC name>` |
| 100.x IPs / MagicDNS | Shown only as "REN-DESKTOP · Direct · 4 ms" |

**Can't hide:** the sign-in web page is served by Tailscale, so it shows their branding for a few seconds. An embedded webview won't work either, because Google blocks sign-in inside embedded webviews.
**User already has Tailscale installed:** detect it and reuse it instead of installing a second copy.

## How Moonlight stays out of sight

- Never open moonlight-qt's main window. Solinear runs `moonlight pair <host> --pin <pin>` and `moonlight stream <host> "Desktop" <quality flags>`.
- The user sees only the fullscreen stream. Moonlight's stats overlay appears only when the player turns it on with a shortcut.
- Not doing in v1: forking moonlight-qt, or a custom client built on `moonlight-common-c`.

## Automatic quality (`quality.py`)

- **Resolution / fps:** match the client's screen. Apollo's virtual display takes that exact size.
- **Codec:** AV1 → HEVC → H.264, the best both ends support.
- **Bitrate:** set from Direct/Relayed and ping (`tailscale ping`). The user's Quality choice scales it up or down.
- Audio, gamepads, mouse: fixed defaults. There is no settings screen in v1.

## Components

1. **Host agent** (`host.py`):
   - listens **only on the Tailscale IP**
   - `GET /info` → host name, apps
   - `POST /pair {pin, name}` → `whois` the caller, check it matches the owner, then forward to Apollo's local `POST /api/pin`
   - shows a "New device paired: X" notice on the host
2. **Client** (`client.py`): list hosts via `tailscale status --json` (peers that answer `/info`) → generate a PIN → run `moonlight pair` and `POST /pair` at the same time → `moonlight stream`.

## No collisions with existing installs

Solinear's copies must run next to a user's own Tailscale / Moonlight / Sunshine / Apollo without touching them: their own ports, their own settings, their own service names.

| App | Default (user's copy) | Solinear's copy |
|---|---|---|
| Apollo / Sunshine | base port `47989`. Ports are offsets from it: HTTPS −5, web UI +1, video +9, control +10, audio +11, RTSP +21 | `port = 57989` in **Solinear's own** `sunshine.conf`, passed to `sunshine.exe` as a bare argument, so all its ports move to `57984`–`58010`. Its config, apps list and state live in `%APPDATA%\Solinear\apollo\` |
| Apollo service | `ApolloService`, whose name is fixed in `sunshinesvc.exe` and in Apollo itself; it starts `Sunshine.exe` with no arguments | **No service in v1.** Solinear starts `sunshine.exe <own conf>` itself, so the user's `ApolloService` is never touched |
| moonlight-qt | settings in the registry | **portable mode**: `portable.dat` in the **current working directory**, so Solinear always launches it with `cwd=` its own Moonlight folder. Connects with `host:57989` |
| tailscaled | service `Tailscale` (name fixed in the code), UDP `41641`, `%ProgramData%\Tailscale`, default pipe `\\.\pipe\ProtectedPrefix\Administrators\Tailscale\tailscaled` | **No service in v1.** Solinear starts it itself with `--socket=\\.\pipe\ProtectedPrefix\Administrators\Solinear\tailscaled` (must stay under the protected prefix) `--statedir=%APPDATA%\Solinear\tailscale --tun=Solinear --port=41642` |

Solinear runs elevated at logon through a scheduled task created by the installer, because `tailscaled` needs admin rights to create its network adapter.
shortcut: no Windows services, so nothing streams until the user logs in and Solinear is running. Move to services later for secure-desktop capture and streaming before login.

All of Solinear's CLI calls pass `--socket=<own pipe>` so they never talk to the user's own `tailscaled`.

**Tailscale is the hard one.** Two Tailscale instances with real network adapters both claim the same `100.64.0.0/10` address range, so their routes conflict. Plan:
1. **v1:** if Tailscale is already installed and signed in, don't start a second copy; use theirs, as long as it is signed in to the same account on every device. If it's signed in to a different account, tell the user plainly; don't change their login.
2. **Later:** run Solinear's copy with `--tun=userspace-networking` (no adapter, so no route conflict) plus a local proxy so Moonlight can reach the host (the same work as the `tsnet` option below).

**Also shared:** Apollo's virtual display driver (SudoVDA) is installed once per machine. If the user already has Apollo, both copies use the same driver, so check that they don't fight over it.

## Stream toolbar (`overlay.py`, PySide6)

A small, mostly transparent, draggable pill that stays on top of the stream. It runs as its own process (`--overlay <ip> <name>`), starts the stream itself, reconnects when a setting changes, and exits when the stream ends. Its position is remembered in `overlay.json`.
- Pill: drag grip · fullscreen/window (sends Ctrl+Alt+Shift+X) · **Gaming mode** (Ctrl+Alt+Shift+M: Moonlight's mouse-capture toggle, with the shortcut printed on the pill) · menu.
- Menu: Desktop › Screen 1–N (Ctrl+Alt+Shift+F1–F12, handled by Apollo/Sunshine on the host) / Quality / Resolution / Frame rate / Window / HDR (these reconnect); Keyboard and shortcuts › Lock / Sign out / Restart / Shut down (restart and shut down ask first) / File Explorer / Task Manager / Control Panel (via the agent's owner-only `/action`); Stream stats; End session.
- Streams default to desktop mouse mode (`--absolute-mouse`) so the cursor can reach the pill. Exclusive Fullscreen draws over it, so Borderless is the default.
- Keys are sent with `SendInput` (scancodes) on Windows and System Events on macOS, which needs the Accessibility permission.
- Later: Ctrl+Alt+Del (needs a SYSTEM service), file transfer, virtual screen create/remove.

## Keys across Mac and Windows (built 2026-11-23, untested on real machines)

Goal: muscle memory from your own keyboard works on the other OS. A Mac user presses Cmd+C on a Windows host and gets Ctrl+C, and Option+Tab gets Alt+Tab. A Windows user presses Ctrl+C on a Mac host and gets Cmd+C.

**How the keys travel (from source):**
- Moonlight sends Windows virtual-key codes. Mac Cmd is sent as Win (`0x5B`), Option as Alt (`0xA4`), Control as Ctrl (`0xA2`).
- Moonlight sends Cmd/Win **only while system-key capture is active** (`keyboard.cpp`: LGUI is dropped otherwise). Capture needs `--capture-system-keys always|fullscreen` *and* an active mouse capture. Today we pass neither flag, so Cmd is dropped and Cmd+C reaches the host as a plain "c".
- On a Mac host, Sunshine maps `0x5B`→Command, `0xA2`→Control, `0xA4`→Option.
- Apollo and Sunshine both have a host-side remap: `keybindings = [from, to, from, to, …]` in the config file. It's applied in the shared `input.cpp` (`map_keycode`), so it works on Windows and Mac hosts. It applies to every client, and it's read only at startup.

**Mappings (single keys only; the remap can't do combos):**

| Client → Host | Remap | Result |
|---|---|---|
| Mac → Windows | Cmd(`5B`,`5C`)→Ctrl(`A2`,`A3`); Control(`A2`)→Win(`5B`) | Cmd+C/V/Z/S/T/W work. Option+Tab = Alt+Tab. Control+X reaches the Win key (Start menu, Win+D, Win+E). |
| Windows → Mac | Ctrl(`11`,`A2`,`A3`)→Cmd(`5B`,`5C`); Win(`5B`)→Control(`A2`) | Ctrl+C/V/Z work. **Ctrl+Tab = Cmd+Tab** (the app switcher). Alt stays Option. Alt+Tab does nothing; that's the combo limit. |
| Same OS | none | unchanged |

**Steps (~40 lines):**
1. `quality.flags`: add `--capture-system-keys always`, so Cmd/Win and Alt/Option+Tab reach the host while Gaming mode has the mouse.
2. Host agent (`host.set_keys`, `KEYMAPS`): new owner-only `POST /keys {"client_os": "mac"|"windows"}`. The agent picks the mapping from the client OS and its own OS, and writes the `keybindings` line into the config. If the line changed, it restarts Apollo/Sunshine (2–3 s; usually only the first time a new kind of device connects).
3. `client.stream()` calls `/keys` (`match_keys`) first, so the restart finishes before Moonlight connects. The toolbar's reconnect does the same.
4. Settings card: a "Match my keyboard" toggle, on by default. Off sends `client_os` = the host's OS, which means no remap.
5. Tests: the mapping table per OS pair; `/keys` is owner-only; an unchanged mapping means no restart.

**Caveats to check on real machines:**
- Does capture count as active in desktop mouse mode (`--absolute-mouse`) after clicking into the stream, or only in Gaming mode? If only in Gaming mode, Cmd+C in desktop mode keeps typing "c".
- On macOS, Moonlight needs **Input Monitoring/Accessibility** for the keyboard grab, or macOS keeps Cmd+Tab and Cmd+Space for itself.
- The remap is per host, not per client. A Mac and a Windows PC streaming the same host *at the same time* would share one mapping. That's rare for one owner, so we accept it. A restart during someone else's stream would drop it, Not handled yet: a restart drops a stream that's already running (`_set_keys` in `main.py`).
- The newer Sunshine mac keyboard path goes through `virtualhid_input`. Confirm it still uses the same VK→`kVK_Command` table as Apollo's `macos/input.cpp`.

## Host setup automation

- Generate a random Apollo admin username and password and set them with `sunshine.exe <conf> --creds <user> <pass>`, which writes them and exits, so the first-run web page is never needed.
- `/api/pin` needs a session cookie from `POST /api/login` first (not Basic auth). It lives on web UI port 57990; `origin_web_ui_allowed = pc` limits it to localhost.
- `dd_resolution_option = auto` and `dd_refresh_rate_option = auto` make the virtual display match the client. This requires Moonlight's `--game-optimization` flag.
- Firewall rule (installer): the Apollo and Solinear programs are reachable only from `100.64.0.0/10`.

## Facts confirmed from source (2026-11-22)

- moonlight-qt `pair`/`stream`/`quit` open a small Qt window, and **always exit 0**, even on failure. Check pairing with `moonlight list <host>`, which exits 0 only when paired.
- `pair --pin` must be exactly 4 digits, and the attempt times out after 10 s.
- `stream` flags: `--resolution WxH --fps N --bitrate <kbps> --video-codec auto|H.264|HEVC|AV1 --display-mode fullscreen --game-optimization`. They apply to that run only and aren't saved.
- `tailscale up --json` prints `{AuthURL, BackendState}` blocks. `status --json` keys are Go field names: `Self`, `Peer`, `User[id].LoginName`, `TailscaleIPs`, `Online`. `whois --json` returns `UserProfile.LoginName`. `ping` prints `via DERP(x)` when relayed, or an IP:port when direct.
- Two tailscaled copies with real adapters both claiming `100.64.0.0/10`: nothing in the code mentions it. Still untested.

## v1 scope

**macOS host (Sunshine, confirmed from source):** `/api/pin` uses HTTP Basic auth (there's no `/api/login`), and requires a `pairing_id`. We get it from `GET /api/pin` → `{"pairings":[{id,name,address}]}`, matching `address` to the asking client's Tailscale IP. Ports, `--creds` and config keys are the same as Apollo. Needs Screen Recording and Accessibility permissions; no virtual display; gamepads need a licensed add-on.

In: a Windows host, a **macOS host (via the user's Sunshine install)**, Windows and **macOS clients**, one owner, Tailscale's hosted service, a custom Headscale server URL as an advanced setting.
Out: macOS/Linux hosts, mobile, sharing with friends (a later use for the Cloudflare/Discord code), multiple users, a settings screen.

**macOS client (v1 shortcuts):** it uses the user's installed Tailscale app and `/Applications/Moonlight.app` instead of bundled copies. Bundling our own daemon on Mac needs root and a system extension, and Moonlight shares the user's settings there. Hosting is hidden on Mac. The screen size comes from `NSScreen` (Retina pixels, max refresh).

## Later

- Tauri or another native shell, if the Python app proves too heavy or too slow to start.
- Embedding Tailscale with `tsnet` (needs a Go helper plus a local `127.0.0.N` proxy) if the background service causes problems.

## Reading list (about one afternoon)

- Apollo: `sunshine.conf` keys, `apps.json`, `/api/pin`, and how to set the admin password
- moonlight-qt: `--help` for `pair`, `stream`, `list`
- Tailscale CLI: `login`, `status --json`, `whois`, `ping`, `--hostname`, `--unattended`, `--login-server`

## Verify before building

Confirmed from source:
- [x] moonlight-qt `pair --pin` exists, and `stream` accepts resolution/fps/bitrate/codec flags
- [x] Apollo admin credentials can be set without the web page (`--creds`)
- [x] `tailscale up --json` gives `AuthURL`
- [x] moonlight-qt portable mode (`portable.dat` in the working directory)
- [x] tailscaled accepts `--socket`, `--statedir`, `--tun` and `--port`; no single-instance lock

Tested on a real PC (2026-11-22; Apollo 0.4.6, moonlight-qt 6.2.0, Tailscale 1.104.7, with the user's own Tailscale + SunshineService running):
- [x] Reuses an existing signed-in Tailscale; `whois` returns the Google login
- [x] Solinear's Apollo on 57989 runs next to an existing Sunshine on 47989 with no port clash
- [x] `--creds`, `/api/login` cookie, `/api/pin` all work
- [x] Full pairing through the agent over the real tailnet IP: `pair()` → True in ~12 s, `moonlight list` confirms
- [x] Moonlight portable mode writes only to its own folder
- [x] pywebview 6.2 runs on Python 3.14 and the UI renders
- [x] Bug found and fixed: Tailscale output is UTF-8, so on non-English Windows (cp950) it must be decoded as UTF-8 explicitly

- [x] Real two-device stream (2026-11-22): Solinear host on Windows → MacBook over Tailscale. Pairing via the agent, then a fullscreen stream (`tests/mac_client.sh` standing in for the client)

- [x] Mac as client: real Solinear app on the MacBook streams this PC
- [x] Mac as host (2026-11-22, Sunshine 2026.914 via Homebrew): Windows Solinear pairs through the agent (`pairing_id` matched by IP, ~21 s) and streams the Mac

Fixed: "Failed to connect" after host restarts. Cause (from source): Sunshine and Apollo only save their host ID (`root.uniqueid` in the state file) after the first successful pairing, and generate a new random one on every start until then. Each new ID left a stale Moonlight entry holding the same address, and Moonlight's lookup picks the first entry matching the address. `host.pin_server_id` now writes a fixed ID at setup, remembers it in Solinear's config, and restores it if the state file is wiped. Verified: real Apollo reports the same ID across restarts. Clients that already have stale entries need them cleared once.

Still needs a real machine:
- [ ] The Windows client (`client.py`) end to end on a second Windows PC
- [ ] Solinear's own tailscaled (elevated) on a PC without Tailscale
- [ ] `whois` login is stable for Google/GitHub/Microsoft/Apple accounts
- [x] `/api/login` body is `{"username","password"}` JSON and needs `Content-Type: application/json`; returns `Set-Cookie: auth=…`; no CSRF check
- [x] `sunshine.exe <conf> --creds u p` argument order (the config path must come before `--creds`)
- [ ] Two tailscaled copies with real adapters running side by side
- [ ] Second Apollo next to an existing one: same SudoVDA driver, no port clash
- [ ] **Virtual display is not wired up yet.** Apollo only creates one with `headless_mode`, an app's virtual-display flag, or when there's no active screen (`process.cpp:238-241`), and the SudoVDA driver comes from Apollo's installer, which we don't run. For now v1 streams the physical monitor.
- [ ] pywebview installs on the Python version we ship with (it needs pythonnet on Windows)
