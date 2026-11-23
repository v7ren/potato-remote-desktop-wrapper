# Potato Remote Desktop Wrapper (Solinear)

A free, open-source remote desktop for gaming. It wraps three open-source projects so they work as one app:

- **[Tailscale](https://tailscale.com)**: the private network between your devices. Your devices find each other by your account, so there are no IPs, ports or port forwarding to set up.
- **[Apollo](https://github.com/ClassicOldSong/Apollo)** / **[Sunshine](https://github.com/LizardByte/Sunshine)**: the streaming server on the PC or Mac you stream *from*.
- **[Moonlight](https://github.com/moonlight-stream/moonlight-qt)**: the low-latency player on the device you stream *to*.

There are no servers of our own. You install it on each device and sign in once. After that, your hosts show up with a **Play** button, and pairing happens automatically (no PINs). Only devices signed in to *your* account can pair.

While you stream, a small toolbar sits at the top of the screen:
- Fullscreen / window
- **Gaming mode** (captures the mouse; `Ctrl+Alt+Shift+M` toggles it)
- Switch screens
- Quality, resolution and frame rate
- Lock, sign out, restart
- Task Manager, End session

Keyboard shortcuts carry across systems: on a Mac streaming a Windows PC, Cmd+C copies. On a Windows PC streaming a Mac, Ctrl+C copies.

The app uses its own ports, so it doesn't clash with a Tailscale, Moonlight, Sunshine or Apollo you already have.

> Status: early (v0.1). Tested Windows ⇄ Mac over Tailscale. Expect rough edges.

## Windows

### Option A: installer (recommended)

1. Download `Solinear.msi` from [Releases](../../releases) and run it. It isn't code-signed yet, so Windows SmartScreen may warn: click **More info → Run anyway**.
2. Solinear starts right away, and again each time you log in, with no admin prompts. The installer also adds firewall rules that only allow your Tailscale network.
3. Click **Sign in** and log in in the browser that opens.
4. On the PC you want to stream **from**, click **Let me stream this PC**.
5. On the device you want to stream **to**, click **Play** next to that PC.

Uninstall from **Settings → Apps**.

### Option A2: portable zip

Download `Solinear-windows-x64.zip`, unzip it anywhere, and run `Solinear\Solinear.exe`. If you don't already have Tailscale running, Windows asks for admin rights; that's needed for the network adapter. Windows Firewall may ask about `Solinear.exe` or `sunshine.exe` the first time. Allow them.

### Option B: from source

You need Python 3.10+.

```powershell
git clone https://github.com/v7ren/potato-remote-desktop-wrapper
cd potato-remote-desktop-wrapper
pip install -r requirements.txt
```

The streaming tools aren't in the repo. Put them in `bin\`; the easiest way is to copy the `bin` folder from the release zip:

```
bin\tailscale\   tailscale.exe, tailscaled.exe, wintun.dll   (Tailscale for Windows)
bin\apollo\      sunshine.exe + its files                     (Apollo for Windows)
bin\moonlight\   Moonlight.exe + its DLLs                     (Moonlight portable for Windows)
```

```powershell
python -m solinear.main
```

### Build the exe yourself

```powershell
python build.py
```

This makes `dist\Solinear\Solinear.exe`, plus `dist\Solinear.msi` if [WiX 5](https://wixtoolset.org) is installed:

```powershell
dotnet tool install --global wix --version 5.0.2
wix extension add -g WixToolset.Util.wixext/5.0.2
wix extension add -g WixToolset.Firewall.wixext/5.0.2
```

## Mac

The Mac version runs from source and uses the apps you install yourself.

1. Install the [Tailscale app](https://tailscale.com/download/mac) and sign in.
2. Install Moonlight (needed to stream *to* the Mac):
   ```bash
   brew install --cask moonlight
   ```
3. *(Only to stream **from** this Mac)* Install Sunshine:
   ```bash
   brew tap LizardByte/homebrew
   brew trust lizardbyte/homebrew
   brew install sunshine
   ```
   The first time, allow **Screen Recording** and **Accessibility** for Sunshine in System Settings → Privacy & Security.
4. Get the code and run it:
   ```bash
   git clone https://github.com/v7ren/potato-remote-desktop-wrapper
   cd potato-remote-desktop-wrapper
   python3 -m pip install -r requirements.txt
   python3 -m solinear.main
   ```

The toolbar's shortcut buttons need **Accessibility** permission for your terminal (or Python). For Cmd+Tab and Cmd+Space to reach the stream in Gaming mode, give **Moonlight** Input Monitoring permission.

## Tests

```bash
python -m unittest tests.test_core
```

## How it works

[PLAN.md](PLAN.md) has the full design: the account check, ports, toolbar and key mapping. [TESTING.md](TESTING.md) has the manual two-device test.

## License

The code in this repo is MIT-licensed (see [LICENSE](LICENSE)). The release zip also contains unmodified builds of these projects, under their own licenses:
- Apollo: GPL-3.0
- Moonlight: GPL-3.0
- Tailscale: BSD-3-Clause

Their source code is at the links above.
