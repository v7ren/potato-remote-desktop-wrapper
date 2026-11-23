"""Build Solinear with PyInstaller.

Windows: dist/Solinear/ plus dist/Solinear.msi via WiX 5 (dotnet tool install --global wix --version 5.0.2).
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
else:
    # dist/msi = the app + bin/, minus this machine's Moonlight identity (Moonlight.ini holds its private key)
    stage = ROOT / "dist" / "msi"
    shutil.rmtree(stage, ignore_errors=True)
    shutil.copytree(ROOT / "dist" / "Solinear", stage)
    shutil.copytree(ROOT / "bin", stage / "bin", ignore=shutil.ignore_patterns(
        "Moonlight Game Streaming Project", "*.log", "cache", "portable.dat"))
    wix = shutil.which("wix") or str(Path.home() / ".dotnet" / "tools" / "wix.exe")
    if Path(wix).exists():
        subprocess.run([wix, "build", "installer.wxs", "-arch", "x64", "-ext", "WixToolset.Util.wixext",
                        "-ext", "WixToolset.Firewall.wixext", "-o", "dist/Solinear.msi"], cwd=ROOT, check=True)
        print("dist/Solinear.msi is ready.")
    else:
        print("WiX not found (dotnet tool install --global wix --version 5.0.2); dist/msi/ is ready.")
