#!/bin/bash
# Stand-in for Solinear's Windows client (client.py), for testing a Solinear host from a Mac.
# Needs: Tailscale signed in to the same account, Moonlight for Mac in /Applications.
# Usage: bash mac_client.sh <host-tailscale-ip> [WxH] [fps]
set -u
HOST="${1:?usage: mac_client.sh <host-tailscale-ip> [WxH] [fps]}"
RES="${2:-1920x1080}"
FPS="${3:-60}"
ML="/Applications/Moonlight.app/Contents/MacOS/Moonlight"
TS="/Applications/Tailscale.app/Contents/MacOS/Tailscale"
ADDR="$HOST:57989"

echo "== 1. Find the host (GET /info)"
curl -s --max-time 3 "http://$HOST:57980/info" || { echo "FAIL: host agent not reachable"; exit 1; }
echo

echo "== 2. Path to host"
"$TS" ping -c 5 "$HOST" | tail -1

if "$ML" list "$ADDR" >/dev/null 2>&1; then
  echo "== 3. Already paired, skipping"
else
  echo "== 3. Pair (a small Moonlight window will appear)"
  PIN=$(printf "%04d" $((RANDOM % 10000)))
  START=$(date +%s)
  "$ML" pair "$ADDR" --pin "$PIN" &
  MLPID=$!
  for _ in $(seq 1 24); do
    sleep 0.5
    R=$(curl -s --max-time 5 -X POST -H 'Content-Type: application/json' \
         -d "{\"pin\":\"$PIN\",\"name\":\"$(hostname -s)\"}" "http://$HOST:57980/pair")
    echo "   /pair -> $R"
    case "$R" in *'"status": true'*) break ;; *different*) echo "FAIL: refused (different account)"; kill $MLPID; exit 1 ;; esac
  done
  wait $MLPID 2>/dev/null
  "$ML" list "$ADDR" >/dev/null 2>&1 && echo "   PAIRED in $(( $(date +%s) - START ))s" || { echo "FAIL: not paired"; exit 1; }
fi

echo "== 4. Stream $RES@$FPS (Ctrl+Option+Shift+S shows stats, Ctrl+Option+Shift+Q quits)"
"$ML" stream "$ADDR" "Desktop" --resolution "$RES" --fps "$FPS" --bitrate 20000 \
  --video-codec auto --display-mode fullscreen --game-optimization
echo "   stream ended (exit $?)"
