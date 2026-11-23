"""Solinear entry point: one window, two jobs (host this PC, play on another)."""
from __future__ import annotations

import atexit
import ctypes
import importlib.util
import json
import socket
import subprocess
import sys
import threading
import time
import webbrowser

from . import client, host, store, tailnet


def ensure_admin():
    if store.MAC:
        return  # Mac uses the user's Tailscale app, so nothing here needs admin
    # only our own tailscaled needs admin (for its network adapter); reusing the user's Tailscale doesn't
    theirs = tailnet.status(sock=None)
    if ctypes.windll.shell32.IsUserAnAdmin() or (theirs and theirs.get("BackendState") == "Running"):
        return
    args = sys.argv[1:] if store.FROZEN else ["-m", "solinear.main", *sys.argv[1:]]
    ctypes.windll.shell32.ShellExecuteW(None, "runas", sys.executable, " ".join(f'"{a}"' for a in args),
                                        str(store.APP), 1)
    sys.exit(0)


class Api:
    """Every public method is callable from ui/index.html. Underscored names are kept private by pywebview."""

    def __init__(self):
        self._cfg = store.load()
        self._window = None
        self._children = []
        self._agent = None
        self._server = None  # our Apollo/Sunshine process
        self._lock = threading.Lock()  # pywebview runs each UI call on its own thread
        if (p := tailnet.start()):
            self._children.append(p)

    def _toast(self, msg: str):
        if self._window:
            self._window.evaluate_js(f"toast({json.dumps(msg)})")

    def _run_host(self, me):
        with self._lock:
            if self._agent:
                return
            handler = host.make_handler(
                owner=me["login"], whois=tailnet.whois,
                submit=lambda pin, name, ip: host.submit_pin(self._cfg, pin, name, ip),
                info=lambda: {"name": me["name"], "solinear": 1, "os": "mac" if store.MAC else "windows",
                              "screens": host.screens()},
                on_paired=lambda name: self._toast(f"New device paired: {name}"),
                cancel=lambda ip: host.cancel_pending(self._cfg, ip),
                action=host.run_action,
                keys=self._set_keys)
            self._agent = host.serve(me["ip"], handler)  # raises if the port is taken; Apollo not started then
            host.setup(self._cfg)  # self-heal: rewrite config and login before each start
            self._server = host.start_apollo()
            self._children.append(self._server)
            threading.Thread(target=self._keep_server_up, daemon=True).start()

    def _keep_server_up(self):
        """Apollo/Sunshine can exit on their own (e.g. started while the PC is locked: no display to capture).
        Without this, the host stays "not answering" until Solinear is restarted."""
        while True:
            time.sleep(5)
            with self._lock:
                if self._server.poll() is not None:
                    self._server = host.start_apollo()
                    self._children.append(self._server)

    def _set_keys(self, client_os) -> bool:
        """Switch the server's key remap for the connecting client; restarts the server if it changed.
        shortcut: a restart drops any other stream running on this host at that moment (rare for one owner)."""
        with self._lock:
            if not host.set_keys(self._cfg, client_os):
                return False
            if self._server and self._server.poll() is None:
                self._server.terminate()
                try:
                    self._server.wait(timeout=10)  # its ports must be free before the new one starts
                except subprocess.TimeoutExpired:
                    self._server.kill()
                    self._server.wait()
            self._server = host.start_apollo()
            self._children.append(self._server)
            return True

    def stop(self):
        for p in self._children:
            if p.poll() is None:
                p.terminate()

    # --- UI calls ---
    def state(self):
        me = tailnet.me()
        error = None
        if me and self._cfg.get("host") and not self._agent and host.server_exe():
            try:
                self._run_host(me)  # host mode waits until we're signed in
            except (OSError, subprocess.SubprocessError) as e:
                error = f"Couldn't start hosting: {e}"
        missing = [name for name, ok in [("Tailscale", tailnet.exe()), ("Moonlight", client.EXE.exists())] if not ok]
        return {"signed_in": bool(me), "name": me and me["name"], "is_host": bool(self._cfg.get("host")),
                "can_host": bool(host.server_exe()),  # Mac needs the user's Sunshine install
                "mac": store.MAC, "missing": missing, "shared_tailscale": tailnet.socket is None, "error": error}

    def sign_in(self):
        url = tailnet.login(socket.gethostname(), self._cfg.get("login_server"))
        if url:
            webbrowser.open(url)
        return bool(url)

    def make_host(self):
        host.setup(self._cfg)
        return self.state()

    def hosts(self):
        out = []
        for h in client.find_hosts():
            direct, ms = tailnet.ping(h["ip"])
            out.append({**h, "direct": direct, "ms": ms and round(ms)})
        return out

    def settings(self):
        return {"quality": self._cfg.get("quality", "auto"),
                "display_mode": self._cfg.get("display_mode", "borderless"),
                "reverse_scroll": bool(self._cfg.get("reverse_scroll", False)),
                "match_keys": bool(self._cfg.get("match_keys", True))}

    def save_settings(self, quality_choice, display_mode, reverse_scroll, match_keys=True):
        self._cfg.update(quality=quality_choice, display_mode=display_mode, reverse_scroll=bool(reverse_scroll),
                         match_keys=bool(match_keys))
        store.save(self._cfg)
        return self.settings()

    def play(self, ip, name=None):
        s = self.settings()
        if not client.wait_for_server(ip):
            return {"ok": False, "error": "That PC's streaming server isn't answering. Is Solinear open on it?"}
        try:
            if not client.is_paired(ip) and not client.pair(ip):
                return {"ok": False, "error": "Couldn't pair with that PC. Is Solinear open on it?"}
        except PermissionError as e:
            return {"ok": False, "error": str(e)}
        if importlib.util.find_spec("PySide6"):
            # the toolbar process starts the stream itself and closes with it
            me = [sys.executable] if store.FROZEN else [sys.executable, "-m", "solinear.main"]
            self._children.append(subprocess.Popen([*me, "--overlay", ip, name or ip], cwd=str(store.APP)))
        else:  # no Qt available: stream without the toolbar
            self._children.append(client.stream(ip, s["quality"], s["display_mode"], s["reverse_scroll"]))
        return {"ok": True}


def main():
    if sys.argv[1:2] == ["--overlay"]:
        from .overlay import run

        return run(sys.argv[2:])

    import webview  # imported here so tests can load this package without pywebview

    ensure_admin()
    api = Api()
    atexit.register(api.stop)
    api._window = webview.create_window("Solinear", str(store.UI / "index.html"), js_api=api,
                                        width=460, height=640, resizable=False)
    webview.start()


if __name__ == "__main__":
    main()
