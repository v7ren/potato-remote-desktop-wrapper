# Solinear Rust app: contract between the pieces

Rewrite of the Python app (`solinear/*.py`, still the reference for behaviour) as a Tauri 2 app.
The Python app keeps working until this one replaces it; never change `solinear/`.

```
app/
  ui/                 plain HTML/CSS/JS, no bundler, no npm. Tauri `withGlobalTauri: true`.
    index.html        main window (dark game-launcher look)
    toolbar.html      stream toolbar pill (transparent, always on top)
    style.css, app.js, toolbar.js, icons/*.svg
  src-tauri/          Rust crate `solinear`
    src/main.rs       Tauri setup, window creation, command registration
    src/store.rs      config.json in %APPDATA%\Solinear (Mac: ~/Library/Application Support/Solinear)
    src/quality.rs    port of quality.py (bitrate_kbps, fit, flags)
    src/keys.rs       KEYMAPS / keymap(client_os, host_is_mac)
    src/apollo.rs     sunshine.conf text, --creds, state.json repair + uniqueid pin, start/restart, watchdog
    src/agent.rs      host agent HTTP: /info /pair /cancel /action /keys /screen (validation is pure + tested)
    src/moonlight.rs  pair/list/stream argv building and process control
    src/net.rs        talks to the network helper (see below); trait so it can be faked in tests
    src/commands.rs   the Tauri commands below
  helper/             Go module `solinear-net` (tsnet), built separately to solinear-net(.exe)
```

## Tauri commands (UI -> Rust). All async, all return JSON.

| command | args | returns |
|---|---|---|
| `state` | - | `{signed_in, account, device_name, is_host, can_host, mac, missing: [str], error: str|null}` |
| `sign_in` | - | `{url}` (UI opens nothing itself; Rust opens the browser) |
| `switch_account` | - | `{url}`: logs the helper out and starts a new login |
| `set_hosting` | `{enabled: bool}` | state |
| `hosts` | - | `[{name, ip, os: "windows"|"mac", online, direct: bool, ms: num|null, screens: [{id, name, w, h, primary}]}]` |
| `settings` | - | `{quality: "auto"|"smoother"|"sharper", display_mode: "borderless"|"fullscreen"|"windowed", reverse_scroll, match_keys}` |
| `save_settings` | same object | settings |
| `play` | `{ip, name, screen: str|null}` | `{ok, error: str|null}` |

Events Rust -> UI: `toast` (string), `state-changed` (no payload: call `state` again).

Toolbar window (`toolbar.html`, label `toolbar`), commands:

| command | args | effect |
|---|---|---|
| `toolbar_info` | - | `{name, ip, os, screens, current_screen, opts: {quality, size, fps, hdr, display_mode}}` |
| `toolbar_combo` | `{key: "x"|"m"|"s"|"q"}` | presses Ctrl+Alt+Shift+key (Mac: Ctrl+Option+Shift) into the stream window |
| `toolbar_switch_screen` | `{id}` | host captures that screen (reconnects if needed) |
| `toolbar_reconnect` | `{quality?, size?, fps?, hdr?, display_mode?}` | restarts the stream with new options |
| `toolbar_action` | `{name: lock|signout|restart|shutdown|explorer|taskmgr|control}` | host action via agent |
| `toolbar_end` | - | ends the stream |
| `toolbar_drag_end` | `{x, y}` | remember position |

Event Rust -> toolbar: `gaming-mode` (bool) so the button state follows Ctrl+Alt+Shift+M.

## Host agent HTTP (unchanged from Python except /screen)

Same JSON bodies and status codes as `solinear/host.py` `make_handler` (400 bad input, 403 not owner).
New: `POST /screen {"id": "\\\\.\\DISPLAY2"}` -> sets `output_name` in sunshine.conf and restarts Apollo if it
changed -> `{"status": true, "restarted": bool}`. `/info.screens[].id` is the Apollo `output_name`
(Windows `\\.\DISPLAYn` as printed by `bin/apollo/tools/dxgi-info.exe`; Mac: CGDirectDisplayID).

The owner check: the caller's Tailscale login must equal the host's own login. How the caller's login is
known depends on the network layer (pending spike): code it behind `net::Whois`.

## Ports
Apollo base 57989 (HTTPS 57984, web 57990, RTSP 58010 TCP; video 57998, control 57999, audio 58000 UDP).
Agent 57980. Use other ports in tests: a Python Solinear is live on this PC on these ports.

## UI requests (added by the UI agent)

| command | args | effect |
|---|---|---|
| `toolbar_resize` | `{w, h}` (CSS px) | resize the toolbar window to fit its content; called on load and whenever a menu opens, closes or changes view (pill only is about 245x50, with a menu about 280x220-350). Keep the top-left corner fixed. |

UI assumptions: `toolbar_reconnect.size` is `"WxH"` (e.g. `"1920x1080"`) or null for "match this screen"; `fps` is a number or null; `toolbar_info.screens[].id` / `current_screen` are the same ids as `hosts[].screens[].id`. `play.screen` is null when the host has one screen. `toolbar_drag_end` is sent with `window.screenX/Y` after a drag.

## Network helper (`helper/`, binary `solinear-net`)

Embeds Tailscale (tsnet): Solinear gets its own login and tailnet, no adapter, no admin rights, independent of
any Tailscale the user runs. Go module, one file `main.go` (+ `main_test.go`); build: `go build -ldflags="-s -w" -o solinear-net.exe .`

Flags: `--state <dir>` (required, tsnet state), `--token <secret>` (required), `--hostname <name>`,
`--control 127.0.0.1:<port>` (local API; `:0` picks one and logs it to stderr), `--login-server <url>` (Headscale),
`--port 41643` (tsnet UDP port). Logs to stderr. Exits when stdin closes (spawn it with a piped stdin and drop the pipe on exit).

Every control request needs `Authorization: Bearer <token>`, else 401. Errors: `{"error": str}` with 4xx/5xx.

| request | response |
|---|---|
| `GET /status` | `{state: "NeedsLogin"\|"Starting"\|"Running", auth_url: str\|null, self: {name, ip, login}\|null, peers: [{name, ip, online, os}]}` (`self` only when Running; `ip` = first 100.x IPv4; peers sorted by name) |
| `POST /login` | `{auth_url}`: starts interactive login if not Running (waits up to ~10s for the URL); `null` if already Running |
| `POST /logout` | `{ok: true}`; also stops host and connect forwarding. Call `/login` afterwards for another account |
| `GET /whois?ip=` | `{login: str\|null}` |
| `GET /ping?ip=` | `{direct: bool, ms: num\|null}` (disco ping, 5s timeout; failure gives `false, null`) |
| `POST /host {enabled, owner, agent, secret}` | `{ok: true}`. Enabled: listens on the tailnet on TCP 57984/57989/57990/58010 and UDP 57998/57999/58000/58002, forwarding each to `127.0.0.1:<same port>`; peers whose login is not `owner` (case-insensitive; whois cached 60s per IP) are dropped. Also tailnet TCP 57980 = reverse proxy to `agent` (`host:port`) that deletes incoming `X-Solinear-*` headers and sets `X-Solinear-Login` (caller login, or empty) and `X-Solinear-Secret`. Non-owners are NOT blocked there: the agent decides (403). UDP: one local socket per remote ip:port, 60s idle timeout. `enabled:false` closes everything. Fails (500) if not on the tailnet yet or a port is taken |
| `POST /connect {ip}` | `{ok: true}`: client side. Listens on `[::1]` (IPv6 only, so a local Apollo holding the IPv4 ports does not clash) on the same TCP/UDP ports and forwards over the tailnet to `<ip>:<same port>`. Replaces the previous target. Point Moonlight at `[::1]:57989` |
| `POST /disconnect` | `{ok: true}` |
| `ANY /peer/<ip>/agent/<path...>` | proxies method, query, body and Content-Type to `http://<ip>:57980/<path>` through tsnet, 10s timeout; returns the agent's status and body (502 `{error}` if unreachable) |
