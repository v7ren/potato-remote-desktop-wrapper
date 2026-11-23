# Two-device stream test (Windows host → MacBook client)

Goal: a real stream from this PC to the MacBook, with Solinear doing all the host-side setup and pairing.
The Mac uses `tests/mac_client.sh` in place of Solinear's client, because the v1 client is Windows-only. It runs the same Moonlight commands and the same `/pair` call as `client.py`.

## Before you start

- **This PC (host):** Tailscale signed in (it is), Python + pywebview installed (done), `bin/` filled (done).
- **MacBook:** Tailscale signed in to the **same account** (it's on your tailnet already).
  - Install Moonlight: `brew install --cask moonlight`, or download the `.dmg` from the moonlight-qt GitHub releases.
  - Copy `tests/mac_client.sh` to the Mac (AirDrop is fine).
- Both devices on any network. They don't need to be on the same Wi-Fi; trying a different network (for example a phone hotspot on the Mac) also tests direct vs relayed.

## Steps

### 1. Start the host (this PC)

```powershell
python -m solinear.main
```

- Click **Let me stream this PC**. You should see "✓ This PC is ready to stream".
- **If Windows Firewall asks** about `python.exe` or `sunshine.exe`: allow **Private** networks only. These are test-only prompts; the installer adds proper rules later.
- Your own SunshineService keeps running on 47989; Solinear's Apollo uses 57989.

### 2. Run the client (MacBook Terminal)

```bash
bash mac_client.sh <host-tailscale-ip> 1920x1080 60
```

`<host-tailscale-ip>` is this PC's Tailscale IP. The script:
1. reaches the host's agent (`/info`)
2. shows the path: direct (an IP:port) or relayed (`DERP(...)`)
3. pairs: Moonlight shows a small window, and the script sends the PIN to `/pair`. **Watch for the "New device paired" toast on the host.**
4. starts a fullscreen stream of this PC's desktop

### 3. While streaming

- Press **Ctrl+Option+Shift+S** on the Mac for Moonlight's stats. Note the codec, the decode and network latency, and any dropped frames.
- Move windows and play a video on the host to check smoothness.
- Quit with **Ctrl+Option+Shift+Q**.

### 4. Second run

Run the script again. It should skip pairing ("Already paired") and go straight to the stream.

## Write down

| Check | Expected | Result |
|---|---|---|
| `/info` reachable | JSON with host name | |
| Path | direct IP:port (relayed is OK, just note it) | |
| `/pair` | `"status": true`, host shows a toast | |
| Pair time | under ~15 s | |
| Stream starts | host desktop fullscreen on the Mac | |
| Codec / latency (stats) | AV1 or HEVC, network latency near ping | |
| Second run | skips pairing | |
| Host's own Sunshine | still running and unaffected | |

## If something fails

- **`/info` not reachable:** the firewall blocked `python.exe`. Allow it, or check the Solinear window is still open.
- **Pairing never returns true:** look at `%APPDATA%\Solinear\apollo\apollo.log` on the host.
- **Stream black or fails:** same log, and the Moonlight error on the Mac. Apollo isn't elevated in this test, so screens like UAC prompts and the login screen won't capture; that's expected for v1.

## Mac client (real Solinear app on the Mac)

1. Copy `dist\solinear-mac.zip` to the Mac (AirDrop).
2. In Mac Terminal:
   ```bash
   cd ~/Downloads && unzip -o solinear-mac.zip -d solinear && cd solinear
   python3 -m pip install --user pywebview
   python3 -m solinear.main
   ```
3. Expected: a window showing "This device: <Mac name>", **no** "Let me stream this PC" button, and this PC listed under Your PCs with a Direct/Relayed badge. Click **Play**.
4. If it says "Install Tailscale/Moonlight first", that app isn't in `/Applications`.

## Mac as host (Windows plays the Mac)

1. On the Mac: `brew tap LizardByte/homebrew && brew install sunshine` (or the Sunshine `.dmg`; needs macOS 15+).
2. Start Solinear on the Mac (same commands as above) → **Let me stream this Mac**.
3. macOS asks for **Screen Recording** and **Accessibility**: allow both, then quit and reopen Solinear.
4. On this PC: `python -m solinear.main` → the Mac appears under Your PCs → **Play**.
5. If pairing fails: check `~/Library/Application Support/Solinear/apollo/apollo.log` on the Mac.

Note: the Mac's own Sunshine (if you run it via `brew services`) stays on 47989; Solinear's runs a separate copy on 57989.

## Cleanup (optional)

- Host: delete `%APPDATA%\Solinear` to reset Solinear's Apollo pairing and config.
- Mac: in the Moonlight app, right-click the host and choose Delete.
