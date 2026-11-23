"""Drives Solinear's own tailscaled, or reuses the user's Tailscale if it's already signed in."""
from __future__ import annotations

import json
import re
import shutil
import subprocess
import time
from pathlib import Path

from . import store

TS = store.BIN / "tailscale"
MAC_APP_CLI = Path("/Applications/Tailscale.app/Contents/MacOS/Tailscale")
OWN_SOCKET = r"\\.\pipe\ProtectedPrefix\Administrators\Solinear\tailscaled"
WG_PORT = 41642  # user's own Tailscale uses 41641

# None = the user's own Tailscale (default socket). On macOS we always use theirs.
socket = None if store.MAC else OWN_SOCKET


def exe() -> str | None:
    if not store.MAC:
        return str(TS / "tailscale.exe")
    # shortcut: Mac relies on the user's Tailscale app (or Homebrew CLI); bundling our own daemon needs root + a system extension
    return str(MAC_APP_CLI) if MAC_APP_CLI.exists() else shutil.which("tailscale")


def _cli(*args, sock=..., timeout=30):
    sock = socket if sock is ... else sock
    if not exe():
        raise FileNotFoundError("Tailscale is not installed")  # OSError: callers already handle it
    cmd = [exe()] + ([f"--socket={sock}"] if sock else []) + list(args)
    # tailscale prints UTF-8; without this, non-English Windows code pages (e.g. cp950) crash decoding
    return subprocess.run(cmd, capture_output=True, encoding="utf-8", errors="replace", timeout=timeout,
                          creationflags=store.NO_WINDOW)


def status(sock=...) -> dict | None:
    if not exe():
        return None
    try:
        return json.loads(_cli("status", "--json", sock=sock, timeout=10).stdout)
    except (json.JSONDecodeError, subprocess.TimeoutExpired, OSError):
        return None


def start():
    """Start our tailscaled unless the user's own Tailscale is already running. Returns the process or None."""
    global socket
    if store.MAC:
        return None
    theirs = status(sock=None)
    if theirs and theirs.get("BackendState") == "Running":
        socket = None
        return None
    state = store.DATA / "tailscale"
    state.mkdir(parents=True, exist_ok=True)
    proc = subprocess.Popen(
        [str(TS / "tailscaled.exe"), f"--socket={OWN_SOCKET}", f"--statedir={state}",
         "--tun=Solinear", f"--port={WG_PORT}"],
        creationflags=store.NO_WINDOW)
    for _ in range(40):  # daemon needs a moment before the pipe exists
        if status() is not None:
            break
        time.sleep(0.25)
    return proc


def parse_json_blocks(lines):
    """`tailscale up --json` prints indented JSON objects one after another; yield each one."""
    buf = []
    for line in lines:
        buf.append(line)
        if line.rstrip() == "}":
            try:
                yield json.loads("".join(buf))
                buf = []
            except json.JSONDecodeError:
                pass


def login(hostname: str, login_server: str | None = None) -> str | None:
    """Return the sign-in URL to open, or None if already signed in."""
    st = status()
    if st and st.get("BackendState") == "Running":
        return None
    cmd = [exe()] + ([f"--socket={socket}"] if socket else []) + ["up", "--json", f"--hostname={hostname}"]
    if not store.MAC:
        cmd.append("--unattended")  # Windows-only flag: stay connected across logoff
    if login_server:
        cmd.append(f"--login-server={login_server}")
    # `up` keeps running until sign-in completes, so leave it in the background
    proc = subprocess.Popen(cmd, stdout=subprocess.PIPE, encoding="utf-8", errors="replace",
                            creationflags=store.NO_WINDOW)
    for block in parse_json_blocks(proc.stdout):
        if block.get("AuthURL"):
            return block["AuthURL"]
        if block.get("BackendState") == "Running":
            return None
        if block.get("Error"):
            raise RuntimeError(block["Error"])
    raise RuntimeError("Sign-in did not start")


def _login_of(st: dict, user_id) -> str | None:
    return ((st.get("User") or {}).get(str(user_id)) or {}).get("LoginName")


def me() -> dict | None:
    st = status()
    if not st or st.get("BackendState") != "Running":
        return None
    s = st["Self"]
    return {"name": s.get("HostName"), "ip": (s.get("TailscaleIPs") or [None])[0],
            "login": _login_of(st, s.get("UserID"))}


def peers() -> list[dict]:
    st = status() or {}
    return [{"name": p.get("HostName"), "ip": p["TailscaleIPs"][0], "login": _login_of(st, p.get("UserID"))}
            for p in (st.get("Peer") or {}).values() if p.get("Online") and p.get("TailscaleIPs")]


def whois(ip: str) -> str | None:
    try:
        return json.loads(_cli("whois", "--json", ip, timeout=10).stdout)["UserProfile"]["LoginName"]
    except (json.JSONDecodeError, KeyError, TypeError, subprocess.TimeoutExpired, OSError):
        return None


_PONG = re.compile(r"via (\S+) in ([\d.]+)(µs|us|ms|s)\b")


def parse_ping(text: str) -> tuple[bool, float | None]:
    """(direct?, latency ms) from the last pong line of `tailscale ping`."""
    found = _PONG.findall(text)
    if not found:
        return False, None
    via, num, unit = found[-1]
    ms = float(num) * {"µs": 0.001, "us": 0.001, "ms": 1, "s": 1000}[unit]
    return not via.startswith(("DERP(", "peer-relay")), ms


def ping(ip: str) -> tuple[bool, float | None]:
    try:
        return parse_ping(_cli("ping", "-c", "5", ip, timeout=15).stdout)
    except (subprocess.TimeoutExpired, OSError):
        return False, None
