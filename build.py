"""Build Solinear with PyInstaller.

Windows: dist/Solinear/ plus the installer via Inno Setup (if `iscc` is on PATH).
  Put the upstream releases in bin/ first (they are not committed):
    bin/tailscale/  tailscale.exe, tailscaled.exe, wintun.dll   (from a Tailscale Windows release)
    bin/apollo/     sunshine.exe + its assets                    (Apollo Windows build)
    bin/moonlight/  Moonlight.exe + its DLLs                     (moonlight-qt Windows portable zip)

macOS: dist/Solinear.app (client only). Users need the Tailscale and Moonlight apps installed.
"""
import os
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).parent
MAC = sys.platform == "darwin"

if not MAC:
    for need in ["tailscale/tailscaled.exe", "apollo/sunshine.exe", "moonlight/Moonlight.exe"]:
        if not (ROOT / "bin" / need).exists():
            sys.exit(f"missing bin/{need}; see the docstring at the top of build.py")

subprocess.run([sys.executable, "-m", "PyInstaller", "--noconfirm", "--windowed", "--name", "Solinear",
                "--add-data", f"{ROOT / 'solinear' / 'ui'}{os.pathsep}ui",
                # the toolbar uses PySide6; pywebview's optional Qt backend would also pull in PyQt5
                "--exclude-module", "PyQt5", "--exclude-module", "PyQt6", "--exclude-module", "qtpy",
                str(ROOT / "run.py")], cwd=ROOT, check=True)

if MAC:
    print("dist/Solinear.app is ready (unsigned: first launch needs right-click > Open).")
elif shutil.which("iscc"):
    subprocess.run(["iscc", str(ROOT / "installer.iss")], cwd=ROOT, check=True)
else:
    print("Inno Setup (iscc) not found; skipped the installer. dist/Solinear/ is ready.")
