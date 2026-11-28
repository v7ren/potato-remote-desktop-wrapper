"""Client side: find the owner's hosts, pair with them, start the stream. Moonlight's own UI never opens."""
from __future__ import annotations

import json
import secrets
import socket
import subprocess
import time
import urllib.error
import urllib.request
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

from . import quality, store, tailnet
from .host import AGENT_PORT, BASE_PORT

if store.MAC:
    # shortcut: Mac uses the installed Moonlight.app and shares its settings; bundle it inside Solinear.app later
    EXE = Path("/Applications/Moonlight.app/Contents/MacOS/Moonlight")
    MOON = None
else:
    MOON = store.BIN / "moonlight"
    EXE = MOON / "Moonlight.exe"


def _moonlight(*args) -> list[str]:
    if MOON:
        (MOON / "portable.dat").touch()  # portable mode looks in the cwd, so we always run with cwd=MOON
    return [str(EXE), *args]


def _addr(ip: str) -> str:
    return f"{ip}:{BASE_PORT}"


def _info(peer: dict) -> dict | None:
    try:
        with urllib.request.urlopen(f"http://{peer['ip']}:{AGENT_PORT}/info", timeout=1.5) as r:
            return {**peer, **json.load(r)}
    except (OSError, ValueError):
        return None


def find_hosts() -> list[dict]:
    peers = tailnet.peers()
    with ThreadPoolExecutor(8) as pool:
        return [h for h in pool.map(_info, peers) if h]


def is_paired(ip: str) -> bool:
    # moonlight's pair/stream always exit 0; `list` is the only honest check
    try:
        r = subprocess.run(_moonlight("list", _addr(ip)), cwd=MOON, capture_output=True, timeout=40,
                           creationflags=store.NO_WINDOW)
    except (subprocess.TimeoutExpired, OSError):
        return False
    return r.returncode == 0


def _post_pin(ip: str, pin: str) -> bool:
    req = urllib.request.Request(f"http://{ip}:{AGENT_PORT}/pair",
                                 json.dumps({"pin": pin, "name": socket.gethostname()}).encode(),
                                 {"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=5) as r:
        return bool(json.load(r).get("status"))


def _cancel_stale(ip: str) -> None:
    # a Sunshine host refuses a new pairing (409) while an old one from us is still pending
    req = urllib.request.Request(f"http://{ip}:{AGENT_PORT}/cancel", b"{}", {"Content-Type": "application/json"})
    try:
        urllib.request.urlopen(req, timeout=5).close()
    except OSError:
        pass  # older hosts don't have /cancel; pairing may still work


def wait_for_server(ip: str, timeout: float = 20) -> bool:
    """The host's streaming server takes a few seconds to (re)start; don't hand Moonlight a closed port."""
    deadline = time.time() + timeout
    while time.time() < deadline:
        try:
            socket.create_connection((ip, BASE_PORT), 2).close()
            return True
        except OSError:
            time.sleep(1)
    return False


def match_keys(ip: str) -> None:
    """Ask the host to remap keys for this device's keyboard (Mac Cmd+C -> Ctrl+C on Windows, and back).
    The host restarts its server when the mapping changes, so wait for it before Moonlight connects."""
    mine = "mac" if store.MAC else "windows"
    client_os = mine if store.load().get("match_keys", True) else ""
    req = urllib.request.Request(f"http://{ip}:{AGENT_PORT}/keys", json.dumps({"client_os": client_os}).encode(),
                                 {"Content-Type": "application/json"})
    try:
        with urllib.request.urlopen(req, timeout=20) as r:
            restarted = json.load(r).get("restarted")
    except (OSError, ValueError):
        return  # older host without /keys: stream without the remap
    if restarted:
        time.sleep(1)
        wait_for_server(ip)


def pair(ip: str) -> bool:
    pin = f"{secrets.randbelow(10_000):04d}"
    _cancel_stale(ip)
    wait_for_server(ip)  # a Windows host restarts its server on /cancel
    proc = subprocess.Popen(_moonlight("pair", _addr(ip), "--pin", pin), cwd=MOON)
    deadline = time.time() + 30  # room for a first-time macOS "Allow incoming connections?" prompt on the host
    ok = False
    try:
        while time.time() < deadline and proc.poll() is None and not ok:
            time.sleep(0.5)  # the server rejects the PIN until moonlight's request is pending
            try:
                ok = _post_pin(ip, pin)
            except urllib.error.HTTPError as e:
                if e.code == 403:
                    raise PermissionError("That PC belongs to a different account.") from e
            except OSError:
                pass
        if ok:
            try:
                proc.wait(timeout=15)  # success quits by itself
            except subprocess.TimeoutExpired:
                pass
        else:
            _cancel_stale(ip)  # don't leave a request that blocks the next try for 5 min
    finally:
        if proc.poll() is None:
            proc.kill()
    return is_paired(ip)


def stream(ip: str, choice: str = "auto", display_mode: str = "borderless", reverse_scroll: bool = False,
           size: tuple[int, int] | None = None, fps: int | None = None, hdr: bool = False) -> subprocess.Popen:
    """size/fps override the client's own screen (toolbar choices); None = match this screen."""
    match_keys(ip)
    sw, sh, sfps = quality.screen()
    if size is None:
        # match the host screen's shape (see quality.fit). shortcut: assumes the primary screen is streamed;
        # after switching to a differently shaped screen from the toolbar, pick a resolution there.
        screens = (_info({"ip": ip}) or {}).get("screens") or []
        main = next((x for x in screens if x.get("primary")), screens[0] if screens else None)
        if main:
            size = quality.fit(main["w"], main["h"], sw, sh)
    w, h = size or (sw, sh)
    fps = fps or sfps
    direct, _ = tailnet.ping(ip)
    kbps = quality.bitrate_kbps(w, h, fps, direct, choice)
    args = quality.flags(w, h, fps, kbps, display_mode, reverse_scroll, hdr)
    return subprocess.Popen(_moonlight("stream", _addr(ip), "Desktop", *args), cwd=MOON)
