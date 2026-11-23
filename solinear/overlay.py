"""Stream toolbar: a small, mostly transparent, draggable pill that stays on top of the Moonlight window.

Runs as its own process (`--overlay <ip> <name>`): it starts the stream, owns Moonlight, reconnects when a
setting changes, and exits when the stream ends. Buttons either send Moonlight/host shortcuts to the stream
window, or call the host's agent (/action) for things like lock or Task Manager.
"""
from __future__ import annotations

import ctypes
import json
import subprocess
import sys
import threading
import time
import urllib.request

from PySide6.QtCore import QByteArray, QPoint, Qt, QTimer, Signal
from PySide6.QtGui import QAction, QActionGroup, QColor, QGuiApplication, QIcon, QPainter, QPen, QPixmap
from PySide6.QtSvg import QSvgRenderer
from PySide6.QtWidgets import (QApplication, QHBoxLayout, QLabel, QMenu, QMessageBox, QToolButton,
                               QVBoxLayout, QWidget)

from . import client, store
from .host import AGENT_PORT

MOD = "Ctrl+Option+Shift" if store.MAC else "Ctrl+Alt+Shift"
POS_FILE = store.DATA / "overlay.json"
INK = "#E8EAEF"

# --- icons: inline SVG, drawn in the toolbar's ink colour --------------------------------------------------
_SVG = {
    "grip": '<circle cx="9" cy="6" r="1.4"/><circle cx="15" cy="6" r="1.4"/><circle cx="9" cy="12" r="1.4"/>'
            '<circle cx="15" cy="12" r="1.4"/><circle cx="9" cy="18" r="1.4"/><circle cx="15" cy="18" r="1.4"/>',
    "full": '<path d="M4 9V4h5M15 4h5v5M20 15v5h-5M9 20H4v-5" fill="none"/>',
    "menu": '<path d="M4 7h16M4 12h16M4 17h16" fill="none"/>',
    "pad": '<path d="M7 9h4M9 7v4M15 9.5h.01M17.5 11.5h.01" fill="none"/>'
           '<path d="M7.5 5h9a5 5 0 0 1 5 5v1.5a3.5 3.5 0 0 1-6.2 2.2L14 12.5h-4l-1.3 1.2A3.5 3.5 0 0 1 2.5 11.5V10'
           'a5 5 0 0 1 5-5z" fill="none"/>',
}


def icon(name: str, px: int = 36) -> QIcon:
    svg = (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" stroke="{INK}" fill="{INK}" '
           f'stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">{_SVG[name]}</svg>')
    pm = QPixmap(px, px)
    pm.fill(Qt.GlobalColor.transparent)
    p = QPainter(pm)
    QSvgRenderer(QByteArray(svg.encode())).render(p)
    p.end()
    return QIcon(pm)


# --- sending Ctrl+Alt+Shift+<key> to the Moonlight window --------------------------------------------------
_WIN_SCAN = {"x": 0x2D, "m": 0x32, "s": 0x1F, "q": 0x10,
             **{f"f{i}": 0x3A + i for i in range(1, 11)}, "f11": 0x57, "f12": 0x58}
_MAC_CODE = {"x": 7, "m": 46, "s": 1, "q": 12, "f1": 122, "f2": 120, "f3": 99, "f4": 118, "f5": 96, "f6": 97,
             "f7": 98, "f8": 100, "f9": 101, "f10": 109, "f11": 103, "f12": 111}


def send_combo(pid: int, key: str) -> None:
    """Focus the stream window and press MOD+key, as if typed. Moonlight handles X/M/S/Q itself; F1-F12 pass
    through to the host, where Apollo/Sunshine switch screens."""
    if store.MAC:  # needs Accessibility permission for the app running Solinear
        subprocess.run(["osascript", "-e", 'tell application "Moonlight" to activate', "-e",
                        f'tell application "System Events" to key code {_MAC_CODE[key]} '
                        'using {control down, option down, shift down}'], capture_output=True, timeout=5)
        return
    _win_focus(pid)
    time.sleep(0.05)
    mods = [0x1D, 0x38, 0x2A]  # Ctrl, Alt, Shift (scancodes, which SDL/Moonlight read)
    seq = [(m, False) for m in mods] + [(_WIN_SCAN[key], False), (_WIN_SCAN[key], True)] + \
          [(m, True) for m in reversed(mods)]
    _win_send(seq)


def _win_focus(pid: int) -> None:
    from ctypes import wintypes

    user32 = ctypes.windll.user32
    found = []
    proc = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)

    def cb(hwnd, _):
        owner = wintypes.DWORD()
        user32.GetWindowThreadProcessId(hwnd, ctypes.byref(owner))
        if owner.value == pid and user32.IsWindowVisible(hwnd):
            found.append(hwnd)
            return False
        return True

    user32.EnumWindows(proc(cb), 0)
    if found:
        user32.SetForegroundWindow(found[0])


def _win_send(seq) -> None:
    from ctypes import wintypes

    class KEYBDINPUT(ctypes.Structure):
        _fields_ = [("wVk", wintypes.WORD), ("wScan", wintypes.WORD), ("dwFlags", wintypes.DWORD),
                    ("time", wintypes.DWORD), ("dwExtraInfo", ctypes.c_size_t)]

    class MOUSEINPUT(ctypes.Structure):  # only here so the union has the right size
        _fields_ = [("dx", wintypes.LONG), ("dy", wintypes.LONG), ("mouseData", wintypes.DWORD),
                    ("dwFlags", wintypes.DWORD), ("time", wintypes.DWORD), ("dwExtraInfo", ctypes.c_size_t)]

    class _U(ctypes.Union):
        _fields_ = [("ki", KEYBDINPUT), ("mi", MOUSEINPUT)]

    class INPUT(ctypes.Structure):
        _fields_ = [("type", wintypes.DWORD), ("u", _U)]

    arr = (INPUT * len(seq))()
    for i, (scan, up) in enumerate(seq):
        arr[i].type = 1  # INPUT_KEYBOARD
        arr[i].u.ki = KEYBDINPUT(0, scan, 0x0008 | (0x0002 if up else 0), 0, 0)  # SCANCODE | KEYUP
    ctypes.windll.user32.SendInput(len(seq), arr, ctypes.sizeof(INPUT))


# --- host agent ----------------------------------------------------------------------------------------------
def host_info(ip: str) -> dict:
    try:
        with urllib.request.urlopen(f"http://{ip}:{AGENT_PORT}/info", timeout=2) as r:
            return json.load(r)
    except (OSError, ValueError):
        return {}


def host_action(ip: str, name: str) -> bool:
    req = urllib.request.Request(f"http://{ip}:{AGENT_PORT}/action", json.dumps({"action": name}).encode(),
                                 {"Content-Type": "application/json"})
    try:
        with urllib.request.urlopen(req, timeout=5) as r:
            return bool(json.load(r).get("status"))
    except (OSError, ValueError):
        return False


# --- the pill ------------------------------------------------------------------------------------------------
MENU_CSS = f"""
QMenu {{ background: rgba(14,16,20,190); color: {INK}; border: 1px solid rgba(255,255,255,90);
         border-radius: 10px; padding: 6px; font-size: 13px; }}
QMenu::item {{ padding: 7px 26px 7px 14px; border-radius: 6px; }}
QMenu::item:selected {{ background: rgba(255,255,255,34); }}
QMenu::item:disabled {{ color: rgba(232,234,239,110); }}
QMenu::separator {{ height: 1px; background: rgba(255,255,255,45); margin: 5px 8px; }}
QMenu::indicator {{ width: 12px; height: 12px; left: 6px; }}
"""
BTN_CSS = f"""
QToolButton {{ color: {INK}; background: transparent; border: none; border-radius: 8px; padding: 4px 6px;
               font-size: 12px; }}
QToolButton:hover {{ background: rgba(255,255,255,30); }}
QToolButton:checked {{ background: rgba(108,140,255,90); }}
QToolButton::menu-indicator {{ image: none; }}
"""


def styled_menu(title: str = "", parent=None) -> QMenu:
    m = QMenu(title, parent)
    m.setWindowFlags(m.windowFlags() | Qt.WindowType.FramelessWindowHint | Qt.WindowType.NoDropShadowWindowHint)
    m.setAttribute(Qt.WidgetAttribute.WA_TranslucentBackground)
    m.setStyleSheet(MENU_CSS)
    return m


class Pill(QWidget):
    started = Signal(object)  # emitted from worker threads; Qt queues it onto the UI thread

    def __init__(self, ip: str, name: str):
        super().__init__(None, Qt.WindowType.FramelessWindowHint | Qt.WindowType.WindowStaysOnTopHint
                         | Qt.WindowType.Tool)
        self.setAttribute(Qt.WidgetAttribute.WA_TranslucentBackground)
        self.setStyleSheet(BTN_CSS)
        self.ip, self.name = ip, name
        cfg = store.load()
        self.opts = {"quality": cfg.get("quality", "auto"), "display_mode": cfg.get("display_mode", "borderless"),
                     "reverse_scroll": bool(cfg.get("reverse_scroll", False)), "size": None, "fps": None,
                     "hdr": False}
        self.info = host_info(ip)
        self.proc = None
        self.restarting = False
        self._drag = None

        row = QHBoxLayout()
        row.setContentsMargins(8, 5, 8, 5)
        row.setSpacing(2)
        grip = QLabel()
        grip.setPixmap(icon("grip", 32).pixmap(16, 16))
        grip.setToolTip("Drag to move")
        row.addWidget(grip)

        full = self._button(icon("full"), f"Fullscreen / window ({MOD}+X)", lambda: self.combo("x"))
        row.addWidget(full)

        self.game = self._button(icon("pad"), f"Gaming mode captures the mouse. {MOD}+M toggles it.",
                                 self.toggle_gaming)
        self.game.setCheckable(True)
        self.game.setText("Gaming mode")
        self.game.setToolButtonStyle(Qt.ToolButtonStyle.ToolButtonTextBesideIcon)
        col = QVBoxLayout()
        col.setSpacing(0)
        col.setContentsMargins(0, 0, 0, 0)
        col.addWidget(self.game)
        hint = QLabel(f"{MOD}+M")
        hint.setStyleSheet("color: rgba(232,234,239,150); font-size: 10px; padding-left: 8px;")
        col.addWidget(hint)
        row.addLayout(col)

        menu_btn = self._button(icon("menu"), "Menu", None)
        menu_btn.setPopupMode(QToolButton.ToolButtonPopupMode.InstantPopup)
        menu_btn.setMenu(self._build_menu())
        row.addWidget(menu_btn)
        self.setLayout(row)

        self.adjustSize()
        self._place()
        self.watch = QTimer(self, interval=1000, timeout=self._check_stream)
        self.started.connect(self._on_started)

    # -- layout and painting --
    def _button(self, ico, tip, fn):
        b = QToolButton()
        b.setIcon(ico)
        b.setToolTip(tip)
        b.setAutoRaise(True)
        if fn:
            b.clicked.connect(fn)
        return b

    def paintEvent(self, _):
        p = QPainter(self)
        p.setRenderHint(QPainter.RenderHint.Antialiasing)
        p.setPen(QPen(QColor(255, 255, 255, 120), 1))
        p.setBrush(QColor(14, 16, 20, 70))  # mostly transparent
        r = self.rect().adjusted(0, 0, -1, -1)
        p.drawRoundedRect(r, r.height() / 2, r.height() / 2)

    def _place(self):
        try:
            x, y = json.loads(POS_FILE.read_text("utf-8"))
            pos = QPoint(x, y)
            if any(s.availableGeometry().contains(pos) for s in QGuiApplication.screens()):
                return self.move(pos)
        except (FileNotFoundError, ValueError, TypeError):
            pass
        g = QGuiApplication.primaryScreen().availableGeometry()
        self.move(g.center().x() - self.width() // 2, g.top() + 8)

    # -- dragging (anywhere on the pill that isn't a button) --
    def mousePressEvent(self, e):
        if e.button() == Qt.MouseButton.LeftButton:
            self._drag = e.globalPosition().toPoint() - self.frameGeometry().topLeft()

    def mouseMoveEvent(self, e):
        if self._drag is not None:
            self.move(e.globalPosition().toPoint() - self._drag)

    def mouseReleaseEvent(self, _):
        if self._drag is not None:
            self._drag = None
            try:
                store.DATA.mkdir(parents=True, exist_ok=True)
                POS_FILE.write_text(json.dumps([self.x(), self.y()]), "utf-8")
            except OSError:
                pass

    # -- menu --
    def _build_menu(self) -> QMenu:
        m = styled_menu(parent=self)
        desk = styled_menu("Desktop", m)
        m.addMenu(desk)  # Qt 6: addMenu(QMenu) returns a QAction, so keep our own reference
        screens = self.info.get("screens") or [{}, {}]
        group = QActionGroup(desk)
        for i, s in enumerate(screens[:12]):
            label = f"Screen {i + 1}" + (f"  ({s['w']}×{s['h']})" if s.get("w") else "")
            a = QAction(label, desk, checkable=True, checked=(i == 0))
            a.triggered.connect(lambda _=False, k=f"f{i + 1}": self.combo(k))
            group.addAction(a)
            desk.addAction(a)
        desk.addSeparator()
        note = desk.addAction("Changes below reconnect the stream")
        note.setEnabled(False)
        self._choice_menu(desk, "Quality", "quality",
                          [("Smoother", "smoother"), ("Auto", "auto"), ("Sharper", "sharper")])
        self._choice_menu(desk, "Resolution", "size",
                          [("Match this screen", None), ("1280×720", (1280, 720)), ("1920×1080", (1920, 1080)),
                           ("2560×1440", (2560, 1440)), ("3840×2160", (3840, 2160))])
        self._choice_menu(desk, "Frame rate", "fps",
                          [("Match this screen", None), ("30 fps", 30), ("60 fps", 60), ("120 fps", 120),
                           ("144 fps", 144)])
        self._choice_menu(desk, "Window", "display_mode",
                          [("Borderless fullscreen", "borderless"), ("Windowed", "windowed"),
                           ("Fullscreen (hides this toolbar)", "fullscreen")])
        hdr = QAction("HDR", desk, checkable=True, checked=self.opts["hdr"])
        hdr.triggered.connect(lambda on: self.reconnect(hdr=on))
        desk.addAction(hdr)

        mac_host = self.info.get("os") == "mac"
        keys = styled_menu("Keyboard and shortcuts", m)
        m.addMenu(keys)
        for label, act, ask in [("Lock", "lock", False), ("Sign out", "signout", True),
                                ("Restart", "restart", True), ("Shut down", "shutdown", True)]:
            keys.addAction(label, lambda a=act, q=ask, l=label: self.action(a, l if q else None))
        keys.addSeparator()
        keys.addAction("Finder" if mac_host else "File Explorer", lambda: self.action("explorer"))
        keys.addAction("Activity Monitor" if mac_host else "Task Manager", lambda: self.action("taskmgr"))
        keys.addAction("System Settings" if mac_host else "Control Panel", lambda: self.action("control"))

        m.addAction(f"Stream stats   {MOD}+S", lambda: self.combo("s"))
        m.addSeparator()
        m.addAction(f"End session   {MOD}+Q", self.end)
        return m

    def _choice_menu(self, parent: QMenu, title: str, key: str, options):
        sub = styled_menu(title, parent)
        parent.addMenu(sub)
        group = QActionGroup(sub)
        for label, value in options:
            a = QAction(label, sub, checkable=True, checked=(self.opts[key] == value))
            a.triggered.connect(lambda _=False, v=value: self.reconnect(**{key: v}))
            group.addAction(a)
            sub.addAction(a)

    # -- actions --
    def combo(self, key: str):
        if self.proc and self.proc.poll() is None:
            send_combo(self.proc.pid, key)

    def toggle_gaming(self):
        # shortcut: this button can drift out of sync if the user presses the shortcut directly
        self.combo("m")

    def action(self, name: str, confirm: str | None = None):
        if confirm:
            box = QMessageBox(QMessageBox.Icon.Question, "Solinear", f"{confirm} {self.name}?",
                              QMessageBox.StandardButton.Yes | QMessageBox.StandardButton.Cancel, self)
            box.setWindowFlag(Qt.WindowType.WindowStaysOnTopHint)
            if box.exec() != QMessageBox.StandardButton.Yes:
                return
        threading.Thread(target=host_action, args=(self.ip, name), daemon=True).start()

    def end(self):
        self.combo("q")
        QTimer.singleShot(4000, self._kill)  # if Moonlight didn't quit by itself

    def _kill(self):
        if self.proc and self.proc.poll() is None:
            self.proc.terminate()

    # -- stream lifecycle --
    def start_stream(self, old=None):
        """Start (or restart) Moonlight off the UI thread: client.stream pings the host first."""
        o = dict(self.opts)

        def work():
            if old is not None:
                try:
                    old.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    old.kill()
            self.started.emit(client.stream(self.ip, o["quality"], o["display_mode"], o["reverse_scroll"],
                                            o["size"], o["fps"], o["hdr"]))

        threading.Thread(target=work, daemon=True).start()

    def _on_started(self, proc):
        self.proc = proc
        self.restarting = False
        self.game.setChecked(False)  # every stream starts in desktop mouse mode
        self.watch.start()
        self.raise_()

    def reconnect(self, **changes):
        self.opts.update(changes)
        self.restarting = True
        old = self.proc
        self.combo("q")
        self.start_stream(old)

    def _check_stream(self):
        if not self.restarting and self.proc and self.proc.poll() is not None:
            QApplication.quit()  # stream ended


def run(argv: list[str]) -> None:
    ip, name = argv[0], (argv[1] if len(argv) > 1 else argv[0])
    app = QApplication(sys.argv[:1])
    app.setQuitOnLastWindowClosed(False)
    pill = Pill(ip, name)
    pill.show()
    pill.start_stream()
    app.exec()
