"""Host side: our own streaming server (Apollo on Windows, Sunshine on macOS) plus the pairing agent
that only answers the owner's devices."""
from __future__ import annotations

import base64
import json
import re
import secrets
import shutil
import socket
import ssl
import subprocess
import threading
import urllib.request
import uuid
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

from . import store

BASE_PORT = 57989  # user's own Sunshine/Apollo uses 47989
WEB_PORT = BASE_PORT + 1
AGENT_PORT = 57980  # below the server's lowest port (BASE_PORT - 5)
APOLLO = store.BIN / "apollo"
HOME = store.DATA / "apollo"  # name kept for existing installs; holds Sunshine's files on macOS too
CONF = HOME / "sunshine.conf"
SERVER_CWD = None if store.MAC else APOLLO
MAC_SUNSHINE = ["/opt/homebrew/bin/sunshine", "/usr/local/bin/sunshine",
                "/Applications/Sunshine.app/Contents/MacOS/sunshine"]


def server_exe() -> str | None:
    if not store.MAC:
        return str(APOLLO / "sunshine.exe")
    # shortcut: Mac uses the user's Sunshine install (Homebrew or .dmg); bundle it inside Solinear.app later
    return next((p for p in MAC_SUNSHINE if Path(p).exists()), None) or shutil.which("sunshine")


# Key remaps so the client's own shortcuts work on the other OS. Moonlight sends Windows virtual-key codes
# (Mac Cmd arrives as Win 0x5B, Option as Alt, Control as Ctrl); the server remaps single keys before
# injecting them. Keyed by (client OS, host OS). Pairs of [from, to].
KEYMAPS = {
    # Mac client on a Windows host: Cmd -> Ctrl (Cmd+C = Ctrl+C), Control -> Win. Option is already Alt.
    ("mac", "windows"): [0x5B, 0xA2, 0x5C, 0xA3, 0xA2, 0x5B, 0x11, 0x5B],
    # Windows client on a Mac host: Ctrl -> Cmd (Ctrl+C = Cmd+C, Ctrl+Tab = app switcher), Win -> Control.
    ("windows", "mac"): [0xA2, 0x5B, 0xA3, 0x5C, 0x11, 0x5B, 0x5B, 0xA2, 0x5C, 0xA3],
}


def keymap(client_os: str) -> list[int]:
    return KEYMAPS.get((client_os, "mac" if store.MAC else "windows"), [])


def _conf_text(keys: list[int] = ()) -> str:
    lines = [
        f"port = {BASE_PORT}",
        "origin_web_ui_allowed = pc",  # web API reachable from this PC only
        f"file_apps = {HOME / 'apps.json'}",
        f"file_state = {HOME / 'state.json'}",
        f"credentials_file = {HOME / 'state.json'}",
        f"pkey = {HOME / 'key.pem'}",
        f"cert = {HOME / 'cert.pem'}",
        f"log_path = {HOME / 'apollo.log'}",
    ]
    if not store.MAC:  # Windows display-device options (match the client's resolution/refresh)
        lines += ["dd_resolution_option = auto", "dd_refresh_rate_option = auto"]
    if keys:
        lines.append("keybindings = [" + ", ".join(f"0x{k:X}" for k in keys) + "]")
    return "\n".join(lines + [""])


def setup(cfg: dict) -> None:
    """First-time host setup. Idempotent."""
    HOME.mkdir(parents=True, exist_ok=True)
    CONF.write_text(_conf_text(keymap(cfg.get("keys_for", ""))), "utf-8")
    apps = HOME / "apps.json"
    if not apps.exists():
        apps.write_text(json.dumps({"env": {}, "apps": [{"name": "Desktop", "image-path": "desktop.png"}]}), "utf-8")
    if "apollo_user" not in cfg:
        cfg["apollo_user"] = "solinear"
        cfg["apollo_pass"] = secrets.token_urlsafe(24)
    # re-applied on every start: if the server's state file is reset or corrupted, its login goes with it
    subprocess.run([server_exe(), str(CONF), "--creds", cfg["apollo_user"], cfg["apollo_pass"]],
                   cwd=SERVER_CWD, check=True, timeout=30, creationflags=store.NO_WINDOW, capture_output=True)
    pin_server_id(cfg)
    cfg["host"] = True
    store.save(cfg)


def pin_server_id(cfg: dict) -> None:
    """Sunshine/Apollo only save their host ID after the first successful pairing, and make a new random one
    on every start until then. Each new ID leaves clients' Moonlight with a stale entry for the same address,
    which later breaks connecting ("Failed to connect"). Write one fixed ID up front, and restore it if the
    state file is ever wiped."""
    state = HOME / "state.json"
    try:
        data = json.loads(state.read_text("utf-8"))
    except FileNotFoundError:
        data = {}
    root = data.setdefault("root", {})
    # `--creds` rewrites this file with every value as a string ("true", "119480064"), and Apollo then
    # crashes reading its paired devices ("type must be boolean, but is string"). Put the types back.
    for dev in root.get("named_devices", []):
        for k, v in dev.items():
            if v in ("true", "false"):
                dev[k] = v == "true"
            elif k == "perm" and isinstance(v, str) and v.isdigit():
                dev[k] = int(v)
    if root.get("uniqueid"):
        cfg["server_id"] = root["uniqueid"]
    else:
        root["uniqueid"] = cfg.setdefault("server_id", str(uuid.uuid4()).upper())
    state.write_text(json.dumps(data, indent=4), "utf-8")


def set_keys(cfg: dict, client_os: str) -> bool:
    """Rewrite the config for this client's keyboard. True if it changed (the server must restart to read it)."""
    if keymap(client_os) == keymap(cfg.get("keys_for", "")):
        return False
    cfg["keys_for"] = client_os
    store.save(cfg)
    CONF.write_text(_conf_text(keymap(client_os)), "utf-8")
    return True


def start_apollo():
    # its log already goes to log_path; don't echo it into the user's terminal
    return subprocess.Popen([server_exe(), str(CONF)], cwd=SERVER_CWD, creationflags=store.NO_WINDOW,
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def pick_pairing(pairings: list, client_ip: str) -> str | None:
    """Sunshine: the pending pairing request that came from this client's address (may be IPv6-mapped)."""
    for p in pairings:
        if str(p.get("address", "")).removeprefix("::ffff:") == client_ip:
            return p.get("id")
    return None


def _call(path, body=None, headers=None, method=None):
    ctx = ssl.create_default_context()
    ctx.check_hostname = False
    ctx.verify_mode = ssl.CERT_NONE  # the server's own self-signed cert, localhost only
    h = {"Content-Type": "application/json", **(headers or {})}
    data = None if body is None else json.dumps(body).encode()
    req = urllib.request.Request(f"https://127.0.0.1:{WEB_PORT}{path}", data, h, method=method)
    return urllib.request.urlopen(req, timeout=5, context=ctx)


def _basic(cfg: dict) -> dict:
    token = base64.b64encode(f"{cfg['apollo_user']}:{cfg['apollo_pass']}".encode()).decode()
    return {"Authorization": "Basic " + token}


def _pending(cfg: dict) -> list:
    with _call("/api/pin", headers=_basic(cfg)) as r:
        return json.load(r).get("pairings", [])


def cancel_pending(cfg: dict, client_ip: str) -> None:
    """Sunshine keeps an unanswered pairing request open for 5 min and refuses a new one from the same
    device (Moonlight error 409). Clear this client's stale request before it tries again."""
    if not store.MAC:
        return  # Apollo has no such lock
    try:
        while (pid := pick_pairing(_pending(cfg), client_ip)):
            _call("/api/pin", {"pairing_id": pid}, _basic(cfg), method="DELETE").close()
    except (OSError, ValueError):
        pass


def submit_pin(cfg: dict, pin: str, name: str, client_ip: str) -> bool:
    """Hand a client's PIN to our server, the same as typing it into its web page."""
    call = _call
    try:
        if store.MAC:  # upstream Sunshine: Basic auth, and the PIN must name the pending request's id
            auth = _basic(cfg)
            pid = pick_pairing(_pending(cfg), client_ip)
            if not pid:
                return False  # Moonlight's request hasn't reached the server yet; client retries
            with call("/api/pin", {"pairing_id": pid, "pin": pin, "name": name}, auth) as r:
                return bool(json.load(r).get("status"))
        # Apollo: cookie from /api/login
        with call("/api/login", {"username": cfg["apollo_user"], "password": cfg["apollo_pass"]}) as r:
            cookie = (r.headers.get("Set-Cookie") or "").split(";")[0]  # "auth=<token>"
        with call("/api/pin", {"pin": pin, "name": name}, {"Cookie": cookie}) as r:
            return bool(json.load(r).get("status"))
    except (OSError, ValueError):  # server down, wrong creds, or no pairing pending yet
        return False


# Host actions the toolbar can trigger. Fixed commands only; the client just names one.
ACTIONS = {
    "windows": {
        "lock": ["rundll32.exe", "user32.dll,LockWorkStation"],
        "signout": ["shutdown", "/l"],
        "restart": ["shutdown", "/r", "/t", "0"],
        "shutdown": ["shutdown", "/s", "/t", "0"],
        "explorer": ["explorer.exe"],
        "taskmgr": ["taskmgr.exe"],
        "control": ["control.exe"],
    },
    "mac": {
        "lock": ["pmset", "displaysleepnow"],  # locks when "require password after sleep" is on (macOS default)
        "signout": ["osascript", "-e", 'tell application "System Events" to log out'],
        "restart": ["osascript", "-e", 'tell application "System Events" to restart'],
        "shutdown": ["osascript", "-e", 'tell application "System Events" to shut down'],
        "explorer": ["open", "-a", "Finder"],
        "taskmgr": ["open", "-a", "Activity Monitor"],
        "control": ["open", "-a", "System Settings"],
    },
}


def run_action(name: str) -> bool:
    cmd = ACTIONS["mac" if store.MAC else "windows"].get(name)
    if not cmd:
        return False
    subprocess.Popen(cmd, creationflags=store.NO_WINDOW)
    return True


def screens() -> list[dict]:
    """Host monitors (pixel sizes), for the toolbar's screen switcher. Best effort: [] if unknown."""
    try:
        if store.MAC:
            from AppKit import NSScreen

            return [{"w": int(s.frame().size.width * s.backingScaleFactor()),
                     "h": int(s.frame().size.height * s.backingScaleFactor())} for s in NSScreen.screens()]
        import ctypes
        from ctypes import wintypes

        ctypes.windll.user32.SetProcessDPIAware()  # real pixels, not scaled
        out = []
        proc = ctypes.WINFUNCTYPE(ctypes.c_int, ctypes.c_void_p, ctypes.c_void_p,
                                  ctypes.POINTER(wintypes.RECT), ctypes.c_ssize_t)

        def cb(_mon, _hdc, rect, _data):
            r = rect.contents
            out.append({"w": r.right - r.left, "h": r.bottom - r.top})
            return 1

        ctypes.windll.user32.EnumDisplayMonitors(None, None, proc(cb), 0)
        return out
    except Exception:  # noqa: BLE001 - screen list is cosmetic; never break /info over it
        return []


def make_handler(owner: str, whois, submit, info, on_paired=lambda name: None, cancel=lambda ip: None,
                 action=lambda name: False, keys=lambda client_os: False):
    """Pairing agent. Only devices signed in to the owner's account may pair or trigger host actions.
    `info` is a dict or a function returning one (so screen lists stay current)."""

    class Handler(BaseHTTPRequestHandler):
        def _send(self, code, obj):
            data = json.dumps(obj).encode()
            self.send_response(code)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

        def do_GET(self):
            if self.path == "/info":
                return self._send(200, info() if callable(info) else info)
            self._send(404, {"error": "not found"})

        def do_POST(self):
            if self.path not in ("/pair", "/cancel", "/action", "/keys"):
                return self._send(404, {"error": "not found"})
            if whois(self.client_address[0]) != owner or not owner:
                return self._send(403, {"error": "This device is signed in to a different account."})
            if self.path == "/cancel":
                cancel(self.client_address[0])  # only ever the caller's own pending request
                return self._send(200, {"status": True})
            length = int(self.headers.get("Content-Length") or 0)
            if length > 4096:
                return self._send(413, {"error": "too large"})
            try:
                body = json.loads(self.rfile.read(length) or b"{}")
            except json.JSONDecodeError:
                return self._send(400, {"error": "bad json"})
            if self.path == "/action":
                name = str(body.get("action", ""))
                if name not in ACTIONS["windows"]:
                    return self._send(400, {"error": "unknown action"})
                return self._send(200, {"status": bool(action(name))})
            if self.path == "/keys":
                client_os = str(body.get("client_os", ""))
                if client_os not in ("", "mac", "windows"):  # "" = no remap
                    return self._send(400, {"error": "unknown os"})
                return self._send(200, {"status": True, "restarted": bool(keys(client_os))})
            pin, name = str(body.get("pin", "")), str(body.get("name", "device"))[:64]
            if not re.fullmatch(r"\d{4}", pin):
                return self._send(400, {"error": "bad pin"})
            ok = submit(pin, name, self.client_address[0])
            if ok:
                on_paired(name)
            self._send(200, {"status": ok})

        def log_message(self, *args):
            pass

    return Handler


class _ExclusiveServer(ThreadingHTTPServer):
    # Python's default SO_REUSEADDR lets a second Solinear bind the same port on Windows and start a second
    # Apollo next to the first. Fail instead, so only one copy ever hosts.
    allow_reuse_address = False

    def server_bind(self):
        if hasattr(socket, "SO_EXCLUSIVEADDRUSE"):  # Windows only
            self.socket.setsockopt(socket.SOL_SOCKET, socket.SO_EXCLUSIVEADDRUSE, 1)
        super().server_bind()


def serve(ip: str, handler) -> ThreadingHTTPServer:
    server = _ExclusiveServer((ip, AGENT_PORT), handler)  # bound to the Tailscale IP only
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server
