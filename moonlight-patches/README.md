# Solinear's patch to Moonlight

On top of moonlight-qt `v6.2.0` (github.com/moonlight-stream/moonlight-qt). Everything is off unless the
launcher sets `SOLINEAR_CONTROL=1`, so the patched build behaves like stock Moonlight otherwise.

With `SOLINEAR_CONTROL=1`, during `Moonlight stream <address> <app>`:

| Direction | Line | Meaning |
|---|---|---|
| out (stdout) | `stage <name>` | a connection stage started |
| out | `connected` | the stream is up; input reaches the host |
| out | `error <text>` | a launch or stage failure, as Moonlight would show it |
| out | `ok <command>` / `unknown <command>` | reply to each command |
| out | `ended` | the stream is over |
| in (stdin) | `combo <name>` | one of Moonlight's own Ctrl+Alt+Shift actions: `fullscreen`, `mouse-mode`, `stats`, `ungrab`, `minimize`, `paste`, `quit` |
| in | `screen <1-12>` | presses Ctrl+Alt+Shift+F<n> on the host through the stream (Sunshine/Apollo: show display n) |
| in | `quit` | end the stream |

Also with `SOLINEAR_CONTROL=1`: only the address given on the command line is used. Moonlight otherwise
probes every address the host reports (LAN, other networks) and can stream around Solinear's forwarder.

Leave stderr alone (not redirected): Moonlight then writes its log to a file as usual.

## Build (Windows x64)

Needs Visual Studio 2022+ C++ build tools, Qt 6.11.2 `win64_msvc2022_64` (aqtinstall: use the commit
moonlight-qt's CI pins; the 3.3.0 release can't read Qt's current index) and the prebuilt libraries.

```bat
git clone --depth 1 --branch v6.2.0 --recurse-submodules --shallow-submodules https://github.com/moonlight-stream/moonlight-qt.git moonlight-src
cd moonlight-src && git am ..\solinear\moonlight-patches\*.patch
powershell -ExecutionPolicy Bypass -File setup-deps.ps1
rem in a "x64 Native Tools" prompt with Qt's bin on PATH:
mkdir ..\moonlight-build && cd ..\moonlight-build
qmake ..\moonlight-src\moonlight-qt.pro CONFIG+=release && nmake release
```

Deploy: `app\release\Moonlight.exe`, `moonlight-src\libs\windows\lib\x64\*.dll`, `AntiHooking.dll` from the
build, the VC++ runtime DLLs, then `windeployqt --release --no-compiler-runtime --no-opengl-sw
--no-system-d3d-compiler --qmldir moonlight-src\app\gui Moonlight.exe`.
