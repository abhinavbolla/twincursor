#!/usr/bin/env bash
# Real input injection on this Mac, checked by reading it back.
#
# A full-screen probe window records the input it receives. A sharer with real injection
# (--inject real) takes a scripted headless guest's input and posts it as real mouse and keyboard
# events, which land in the probe. `tc-spike replay --probe` then checks that the text, shortcuts,
# clicks and scrolls the probe got are the ones the guest sent.
#
# Run it with the screen unlocked and hands off the mouse and keyboard for about 15 s. The
# terminal needs the Accessibility permission (System Settings → Privacy & Security →
# Accessibility). While the screen is locked nothing is injected, and the check fails safely.
#
# Needs `livekit-server --dev` running and a release build:
#   cargo build --release -p twincursor-media-spike
# Usage: bench/control-inject.sh [script.json]   (default: bench/control/inject/probe.json)
set -euo pipefail

cd "$(dirname "$0")/.."
BIN=${BIN:-target/release/tc-spike}
SCRIPT=${1:-bench/control/inject/probe.json}
OUT=bench/results/control-inject-$(date +%s)
mkdir -p "$OUT"
room="inject-$$"

"$BIN" probe --out "$OUT/probe.jsonl" --duration 40 >"$OUT/probe.log" 2>&1 &
probe=$!
sleep 2

mkfifo "$OUT/answer"
"$BIN" share --audio off --room "$room" --capture pattern --resolution 720 --no-stats \
  --allow-control --inject real <"$OUT/answer" >"$OUT/share.log" 2>&1 &
sharer=$!
exec 3>"$OUT/answer"

"$BIN" view --audio off --room "$room" --headless --no-stats --duration 60 --label inject --out "$OUT" \
  --name "inject check" --control-script "$SCRIPT" --sent-log "$OUT/sent.jsonl" \
  >"$OUT/view.log" 2>&1 &
viewer=$!

for _ in $(seq 120); do
  grep -q "asks to control this computer" "$OUT/share.log" && break
  kill -0 "$viewer" 2>/dev/null || break
  sleep 0.5
done
echo y >&3

viewer_ok=1
wait "$viewer" || viewer_ok=0
kill -INT "$sharer" 2>/dev/null || true
wait "$sharer" 2>/dev/null || true
exec 3>&-
rm -f "$OUT/answer"
kill "$probe" 2>/dev/null || true
wait "$probe" 2>/dev/null || true

if grep -q "screen is locked" "$OUT/share.log"; then
  echo "FAIL: the screen was locked, so nothing was injected (logs in $OUT)"
  exit 1
fi
if [[ $viewer_ok -eq 1 ]] && "$BIN" replay --sent "$OUT/sent.jsonl" --probe "$OUT/probe.jsonl"; then
  echo "inject: PASS"
else
  echo "inject: FAIL (logs in $OUT)"
  tail -3 "$OUT/share.log" "$OUT/view.log"
  exit 1
fi
