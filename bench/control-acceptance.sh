#!/usr/bin/env bash
# The remote-control checks that need someone at the Mac: an unlocked screen, and hands off the
# mouse and keyboard for about two minutes. Everything else runs headless in bench/control-e2e.sh.
#
#   1. The revoke hotkey, pressed for real mid-typing (REVOKE=hotkey bench/control-e2e.sh).
#   2. Real injection read back by a full-screen probe window (bench/control-inject.sh).
#   3. The overlay: a screenshot of your screen during control (label and name tag visible), and
#      a frame of what the guest received (neither visible).
#   4. Capture latency with and without --allow-control, which leaves the app's windows out.
#
# The terminal needs Accessibility and Screen Recording permission, and `livekit-server --dev`
# must be running. Results land in bench/results/acceptance-<time>/.
# STEPS="2 3" bench/control-acceptance.sh runs only those steps.
set -uo pipefail

cd "$(dirname "$0")/.."
BIN=${BIN:-target/release/tc-spike}
OUT=bench/results/acceptance-$(date +%s)
mkdir -p "$OUT"
status=0
note() { echo; echo "=== $*"; }
want() { [[ " ${STEPS:-1 2 3 4} " == *" $1 "* ]]; }

if want 1; then
note "1. Revoke hotkey"
REVOKE=hotkey bench/control-e2e.sh || status=1
fi

if want 2; then
note "2. Real injection, read back"
bench/control-inject.sh || status=1
fi

if want 3; then
note "3. Overlay on screen, not in the stream"
room="accept-overlay-$$"
mkfifo "$OUT/answer"
"$BIN" share --room "$room" --capture sck --resolution 1080 --no-stats --allow-control \
  --inject log <"$OUT/answer" >"$OUT/share.log" 2>&1 &
sharer=$!
exec 3>"$OUT/answer"
cat >"$OUT/script.json" <<'JSON'
[{"do": "request"}, {"do": "input", "event": {"kind": "move", "x": 0.5, "y": 0.5}},
 {"do": "wait", "ms": 4000}, {"do": "release"}]
JSON
"$BIN" view --room "$room" --headless --no-stats --duration 30 --out "$OUT" --name "Acceptance" \
  --control-script "$OUT/script.json" --snapshot-every 1 --snapshot-dir "$OUT/stream" \
  >"$OUT/view.log" 2>&1 &
viewer=$!
for _ in $(seq 60); do grep -q "asks to control" "$OUT/share.log" && break; sleep 0.5; done
echo y >&3
for _ in $(seq 40); do grep -q "now controls this computer" "$OUT/share.log" && break; sleep 0.25; done
sleep 1.5
screencapture -x "$OUT/overlay-on-screen.png"
wait "$viewer"
kill -INT "$sharer" 2>/dev/null
wait "$sharer" 2>/dev/null
exec 3>&-
rm -f "$OUT/answer"
if grep -q "leaving this app's own windows out of the capture" "$OUT/share.log"; then
  echo "PASS: the sharer left its own windows out of the capture"
else
  echo "FAIL: the sharer didn't find its own windows, so the overlay is in the stream"
  status=1
fi
echo "Your screen during control: $OUT/overlay-on-screen.png (the label and name tag should show)"
echo "What the guest received: the newest PGM in $OUT/stream/ (neither should show; the"
echo "  black and white blocks over the top-left menus are the latency marker)"
fi

if want 4; then
note "4. Capture latency with and without --allow-control (25 s each)"
for mode in plain control; do
  flags=""
  [[ $mode == control ]] && flags="--allow-control --inject log"
  room="accept-lat-$mode-$$"
  # shellcheck disable=SC2086
  "$BIN" share --room "$room" --capture sck --resolution 1080 $flags </dev/null >"$OUT/lat-$mode.log" 2>&1 &
  sharer=$!
  for _ in $(seq 60); do grep -q "screen track published" "$OUT/lat-$mode.log" && break; sleep 0.5; done
  sleep 3
  "$BIN" view --room "$room" --headless --duration 25 --label "lat-$mode" --out "$OUT" >/dev/null 2>&1
  kill -INT "$sharer" 2>/dev/null
  wait "$sharer" 2>/dev/null
done
for f in "$OUT"/lat-*.json; do
  jq -r '"\(.label): capture → decode p50 \(.report.one_way_presented.p50 | . * 10 | round / 10) ms, p95 \(.report.one_way_presented.p95 | . * 10 | round / 10) ms, \(.avg_fps | . * 10 | round / 10) fps"' "$f"
done
fi

echo
if [[ $status -eq 0 ]]; then echo "Automatic checks passed. Look at the two images in step 3."; else echo "Some checks FAILED: see above."; fi
exit $status
