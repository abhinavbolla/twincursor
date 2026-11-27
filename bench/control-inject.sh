#!/usr/bin/env bash
# Real input injection on this Mac, checked by reading it back.
#
# A full-screen probe window (`tc-spike probe`) records the input it receives. A sharer with real
# injection (--inject real) takes a scripted headless guest's input and posts it as real mouse and
# keyboard events, which land in the probe. `tc-spike replay --probe` then checks that the text,
# shortcuts, clicks and scrolls the probe got are the ones the guest sent.
#
# Real input goes wherever the pointer and the key window are, so nothing goes out until there is
# evidence it will land in the probe, and the run stops at the first sign it isn't:
#   1. The probe opens on the display the injector targets (the injector's own function says
#      which) and records `ready` once it is the key window of the frontmost app, covering that
#      display. No `ready` within a few seconds: FAIL, and no sharer is ever started.
#   2. The sharer and the guest start; the guest asks for control and is let in.
#   3. The guest sends one pointer move (the canary) and waits at a gate. Only once the probe has
#      recorded that move where it was aimed does the bench open the gate.
#   4. Moves, clicks, a right-click and scrolls, then a second gate: opened only once the probe
#      has recorded every click, in order and in place.
#   5. Last, the text (with Enter), ⌘A and ⌃C.
# A watcher takes control back the moment the probe records `lost_focus` (or closes), and any
# failed check aborts: control is revoked through the sharer's prompt, the sharer is stopped
# (killed if it hangs), and only then are the guest and the probe closed. The FAIL message says
# what had been sent at most.
#
# Run it with the screen unlocked and hands off the mouse and keyboard for about 20 s. Nothing may
# be full-screen on the display with the menu bar (the main display), which is where the probe
# opens and where input goes. The terminal needs the Accessibility permission (System Settings →
# Privacy & Security → Accessibility). The probe refuses to open while the screen is locked.
#
# Dry run, with no real input at all: INJECT=record:FILE records what the sharer would inject.
# The probe can't see recorded input, so the canary never arrives and the run must abort with
# FILE holding only the canary move; the bench checks that and exits 0. DRY_FAKE writes fake
# probe records to take the run further: `canary` (fake the canary, then abort at the clicks
# with no keys sent), `canary,lost` (then a fake lost_focus), `canary,buttons` (the whole
# script, checked with `replay --actions`).
#
# Needs `livekit-server --dev` running and a release build:
#   cargo build --release -p twincursor-media-spike
# Usage: bench/control-inject.sh [script.json]   (default: bench/control/inject/probe.json; it
# must have the canary and the gates, as the default does, or the probe-gate check refuses it)
set -euo pipefail

cd "$(dirname "$0")/.."
BIN=${BIN:-target/release/tc-spike}
SCRIPT_SRC=${1:-bench/control/inject/probe.json}
INJECT=${INJECT:-real}
DRY_FAKE=${DRY_FAKE:-}
OUT=bench/results/control-inject-$(date +%s)-$$
mkdir -p "$OUT"
OUT=$(cd "$OUT" && pwd)
room="inject-$$"
PROBE="$OUT/probe.jsonl"
SCRIPT="$OUT/script.json"

case $INJECT in
  real) dry=0 ;;
  record:?*) dry=1; RECORD=${INJECT#record:} ;;
  *) echo "INJECT must be real or record:FILE"; exit 1 ;;
esac
if [[ -n $DRY_FAKE && $dry -eq 0 ]]; then
  echo "DRY_FAKE writes fake probe records, so it only works with INJECT=record:FILE"
  exit 1
fi
fake() { [[ ",$DRY_FAKE," == *",$1,"* ]]; }
sed "s|@OUT@|$OUT|g" "$SCRIPT_SRC" >"$SCRIPT"
gate() { "$BIN" probe-gate "$1" --probe "$PROBE" --script "$SCRIPT" --pid "$probe" --timeout "$2"; }

probe="" sharer="" viewer="" watcher=""
# What has been let out so far, at most. Updated before each step that sends more.
sent="nothing"

# Takes control back and stops the sharer, which is the only thing that injects. Revoking goes
# first: it blocks injection at once and releases anything held; then the sharer is asked to
# stop, and killed if it hasn't within 2 s.
stop_sharer() {
  [[ -n $sharer ]] || return 0
  if kill -0 "$sharer" 2>/dev/null; then
    echo r >&3 2>/dev/null || true
    kill -INT "$sharer" 2>/dev/null || true
    for _ in $(seq 20); do kill -0 "$sharer" 2>/dev/null || break; sleep 0.1; done
    kill -KILL "$sharer" 2>/dev/null || true
  fi
  wait "$sharer" 2>/dev/null || true
  sharer=""
}

# Runs on every exit, aborts included: the sharer first, then everything else.
cleanup() {
  trap - USR1
  stop_sharer
  for g in gate-canary gate-keys; do [[ -s $OUT/$g ]] || echo stop >"$OUT/$g"; done
  [[ -n $watcher ]] && pkill -P "$watcher" 2>/dev/null || true
  for p in $watcher $viewer $probe; do kill "$p" 2>/dev/null || true; done
  for p in $watcher $viewer $probe; do wait "$p" 2>/dev/null || true; done
  exec 3>&- 2>/dev/null || true
  rm -f "$OUT/answer"
}
trap cleanup EXIT
trap 'exit 130' INT TERM

# The recorded actions in a dry run, one word each (move_to, button, scroll, key, text).
recorded() { [[ -s $RECORD ]] && sed -E 's/.*"action":"([a-z_]+)".*/\1/' "$RECORD" | sort | uniq -c | tr '\n' ' '; }

abort() {
  stop_sharer
  echo "inject: FAIL: $1"
  echo "  sent at most: $sent. Logs in $OUT"
  if [[ $dry -eq 1 ]]; then
    echo "  dry run, recorded instead of injected: $(recorded)"
    dry_check "$2"
    exit $?
  fi
  exit 1
}

# In a dry run, an abort is the expected end; check it came where it should, with nothing
# beyond what that point allows in the record file.
dry_check() {
  local at=$1 bad
  case $at in
    canary)
      [[ -z $DRY_FAKE ]] || { echo "dry run: FAIL (aborted at the canary despite DRY_FAKE)"; return 1; }
      bad=$(grep -cv '"action":"move_to"' "$RECORD" || true)
      moves=$(grep -c '"action":"move_to"' "$RECORD" || true)
      if [[ $bad -eq 0 && $moves -eq 1 ]]; then
        echo "dry run: PASS (aborted before the main script; the record holds only the canary move)"
        return 0
      fi
      echo "dry run: FAIL (expected only the canary move, got $moves moves and $bad other actions)"
      return 1
      ;;
    buttons | lost)
      if { [[ $at == lost ]] && ! fake lost; } || { [[ $at == buttons ]] && ! fake canary; } || fake buttons; then
        echo "dry run: FAIL (aborted at '$at', which DRY_FAKE='$DRY_FAKE' doesn't call for)"
        return 1
      fi
      bad=$(grep -cE '"action":"(key|text)"' "$RECORD" || true)
      if [[ $bad -eq 0 ]]; then
        echo "dry run: PASS (aborted at '$at' with no keys or text sent)"
        return 0
      fi
      echo "dry run: FAIL ($bad keys or text went out before the keys gate)"
      return 1
      ;;
  esac
  echo "dry run: FAIL (aborted at an unexpected point: $at)"
  return 1
}

# A fake probe record, for DRY_FAKE.
fake_line() { echo "$1" >>"$PROBE"; }

# 1. The probe, on the display input goes to, ready before anything else starts.
"$BIN" probe --out "$PROBE" --duration 90 --ready-timeout 6 >"$OUT/probe.log" 2>&1 &
probe=$!
if ! why=$(gate ready 10); then
  echo "inject: FAIL: the test window didn't get focus; nothing was injected"
  echo "  $why"
  tail -2 "$OUT/probe.log" | sed 's/^/  /'
  exit 1
fi
echo "probe: $why"

# 2. The sharer and the guest.
mkfifo "$OUT/answer"
"$BIN" share --audio off --room "$room" --capture pattern --resolution 720 --no-stats \
  --allow-control --inject "$INJECT" <"$OUT/answer" >"$OUT/share.log" 2>&1 &
sharer=$!
exec 3>"$OUT/answer"

# From here on, the probe losing focus or closing takes control back at once, whatever the
# main line is doing.
(
  "$BIN" probe-gate watch --probe "$PROBE" --script "$SCRIPT" --pid "$probe" --timeout 0 >"$OUT/watch.log" 2>&1 || true
  echo r >&3 2>/dev/null || true
  kill -INT "$sharer" 2>/dev/null || true
  touch "$OUT/lost"
  kill -USR1 $$ 2>/dev/null || true
) &
watcher=$!
trap 'abort "$(cat "$OUT/watch.log" 2>/dev/null)" lost' USR1

"$BIN" view --audio off --room "$room" --headless --no-stats --duration 90 --label inject --out "$OUT" \
  --name "inject check" --control-script "$SCRIPT" --sent-log "$OUT/sent.jsonl" \
  >"$OUT/view.log" 2>&1 &
viewer=$!

asked=0
for _ in $(seq 120); do
  grep -q "asks to control this computer" "$OUT/share.log" && { asked=1; break; }
  kill -0 "$viewer" 2>/dev/null || break
  sleep 0.5
done
[[ $asked -eq 1 ]] || abort "the guest never asked for control" setup
# Still ready right before control is given? (`watch` times out, exit 2, when all is well.)
set +e
why=$(gate watch 0.3)
rc=$?
set -e
[[ $rc -eq 2 && ! -e $OUT/lost ]] || abort "the test window lost focus before control was given: $why" lost
sent="the canary pointer move only, no clicks, no keys"
echo y >&3
for _ in $(seq 40); do grep -q "now controls this computer" "$OUT/share.log" && break; sleep 0.25; done
grep -q "now controls this computer" "$OUT/share.log" ||
  abort "the sharer didn't give control: $(tail -1 "$OUT/share.log")" setup

# 3. The canary.
if fake canary; then
  sleep 1
  read -r _ cx cy < <("$BIN" probe-gate plan --probe "$PROBE" --script "$SCRIPT" | grep '^canary')
  fake_line "{\"kind\":\"move\",\"x\":$cx,\"y\":$cy}"
fi
if ! why=$(gate canary 4); then
  abort "the canary move didn't reach the test window ($why), so the rest wasn't sent" canary
fi
echo "canary: $why"

# 4. Moves, clicks, scrolls.
sent="pointer moves, clicks, a right-click and scrolls, no keys"
echo go >"$OUT/gate-canary"
if fake lost; then
  sleep 0.3
  fake_line '{"lost_focus":{"on_target":true,"key":false,"frontmost":false,"frame":null}}'
  sleep 2
fi
if fake buttons; then
  sleep 1.5
  "$BIN" probe-gate plan --probe "$PROBE" --script "$SCRIPT" | grep '^button' |
    while read -r _ button pressed x y; do
      fake_line "{\"kind\":\"button\",\"button\":$button,\"pressed\":$pressed,\"x\":$x,\"y\":$y}"
    done
fi
if ! why=$(gate buttons 8); then
  [[ -e $OUT/lost ]] && abort "$(cat "$OUT/watch.log")" lost
  abort "the clicks didn't all reach the test window in place ($why), so no keys were sent" buttons
fi
echo "clicks: $why"

# 5. Text, ⌘A, ⌃C.
sent="the whole script"
echo go >"$OUT/gate-keys"

viewer_ok=1
wait "$viewer" || viewer_ok=0
viewer=""
[[ -e $OUT/lost ]] && abort "$(cat "$OUT/watch.log")" lost
# Done: the watcher goes before the probe closes, so closing isn't taken for losing focus.
trap - USR1
pkill -P "$watcher" 2>/dev/null || true
kill "$watcher" 2>/dev/null || true
wait "$watcher" 2>/dev/null || true
watcher=""
stop_sharer

if grep -q "screen is locked" "$OUT/share.log"; then
  echo "FAIL: the screen was locked, so nothing was injected (logs in $OUT)"
  exit 1
fi
if [[ $dry -eq 1 ]]; then
  # Nothing reached the probe; check the record against what was sent instead.
  if [[ $viewer_ok -eq 1 ]] && "$BIN" replay --sent "$OUT/sent.jsonl" --actions "$RECORD"; then
    echo "dry run: PASS (the whole script went through the gates; recorded: $(recorded))"
    exit 0
  fi
  echo "dry run: FAIL (logs in $OUT)"
  exit 1
fi
kill "$probe" 2>/dev/null || true
wait "$probe" 2>/dev/null || true
probe=""
if [[ $viewer_ok -eq 1 ]] && "$BIN" replay --sent "$OUT/sent.jsonl" --probe "$PROBE"; then
  echo "inject: PASS"
else
  echo "inject: FAIL (logs in $OUT)"
  tail -3 "$OUT/share.log" "$OUT/view.log"
  exit 1
fi
