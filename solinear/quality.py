"""Picks every stream setting, so users never have to."""
from __future__ import annotations

import ctypes
import sys

SCALE = {"auto": 1.0, "smoother": 0.7, "sharper": 1.5}
RELAY_CAP = 10_000  # kbps; relayed paths can't carry much more


def screen() -> tuple[int, int, int]:
    """Client's real resolution and refresh rate."""
    if sys.platform == "darwin":
        from AppKit import NSScreen  # pyobjc, installed with pywebview on macOS

        s = NSScreen.mainScreen()
        k = s.backingScaleFactor()  # Retina: points -> pixels
        hz = s.maximumFramesPerSecond() if hasattr(s, "maximumFramesPerSecond") else 60
        return int(s.frame().size.width * k), int(s.frame().size.height * k), int(hz) or 60
    user32, gdi32 = ctypes.windll.user32, ctypes.windll.gdi32
    user32.SetProcessDPIAware()  # otherwise Windows reports scaled-down sizes
    w, h = user32.GetSystemMetrics(0), user32.GetSystemMetrics(1)
    hdc = user32.GetDC(0)
    hz = gdi32.GetDeviceCaps(hdc, 116)  # VREFRESH
    user32.ReleaseDC(0, hdc)
    return w, h, hz if hz > 1 else 60


def fit(host_w: int, host_h: int, client_w: int, client_h: int) -> tuple[int, int]:
    """Stream at the host screen's shape. Without a virtual display the host can't switch to the client's
    shape (a 3:2 Mac on a 16:9 PC), so the server pads the picture and absolute mouse positions land off
    target, more so toward the edges. Shrink to fit the client screen; never upscale. Even sizes for the encoder."""
    k = min(1.0, client_w / host_w, client_h / host_h)
    return int(host_w * k) // 2 * 2, int(host_h * k) // 2 * 2


def bitrate_kbps(w: int, h: int, fps: int, direct: bool, quality: str = "auto") -> int:
    kbps = w * h * fps * 0.16 / 1000 * SCALE.get(quality, 1.0)  # ~20 Mbps at 1080p60
    if not direct:
        kbps = min(kbps, RELAY_CAP)
    return int(max(2_000, min(kbps, 150_000)))


DISPLAY_MODES = ("borderless", "fullscreen", "windowed")


def flags(w: int, h: int, fps: int, kbps: int, display_mode: str = "borderless",
          reverse_scroll: bool = False, hdr: bool = False) -> list[str]:
    if display_mode not in DISPLAY_MODES:
        display_mode = "borderless"
    return ["--resolution", f"{w}x{h}", "--fps", str(fps), "--bitrate", str(kbps),
            "--video-codec", "auto", "--display-mode", display_mode,
            "--reverse-scroll-direction" if reverse_scroll else "--no-reverse-scroll-direction",
            "--hdr" if hdr else "--no-hdr",
            # desktop mouse: the cursor stays free, so the toolbar is reachable. Gaming mode (captured
            # mouse) is toggled live with Ctrl+Alt+Shift+M, which the toolbar shows.
            "--absolute-mouse",
            # send Cmd/Win and Alt/Option+Tab to the host while the stream has the keyboard (Gaming mode)
            "--capture-system-keys", "always",
            "--game-optimization"]  # Apollo only resizes its virtual display with this on
