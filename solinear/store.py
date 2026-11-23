"""Paths and the local config file. Nothing here ever leaves this PC."""
from __future__ import annotations

import json
import os
import subprocess
import sys
from pathlib import Path

MAC = sys.platform == "darwin"
FROZEN = getattr(sys, "frozen", False)
APP = Path(sys.executable).parent if FROZEN else Path(__file__).resolve().parent.parent
BIN = APP / "bin"  # bundled tailscale/, apollo/, moonlight/
UI = Path(getattr(sys, "_MEIPASS", APP / "solinear")) / "ui"
DATA = (Path.home() / "Library" / "Application Support" / "Solinear" if MAC
        else Path(os.environ.get("APPDATA", Path.home())) / "Solinear")
CONFIG = DATA / "config.json"

NO_WINDOW = getattr(subprocess, "CREATE_NO_WINDOW", 0)


def load() -> dict:
    try:
        return json.loads(CONFIG.read_text("utf-8"))
    except (FileNotFoundError, json.JSONDecodeError):
        return {}


def save(cfg: dict) -> None:
    DATA.mkdir(parents=True, exist_ok=True)
    tmp = CONFIG.with_suffix(".tmp")
    tmp.write_text(json.dumps(cfg, indent=2), "utf-8")
    tmp.replace(CONFIG)  # atomic, so a crash never leaves a half-written config
