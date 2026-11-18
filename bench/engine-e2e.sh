#!/usr/bin/env bash
# The engine as the desktop app drives it, end to end on one machine, with nobody at the Mac.
#
# Plays the app's part: starts `tc-engine share --ipc` (synthetic pattern, --allow-control,
# recording every action it would inject) and a headless `tc-engine view --ipc`, writes commands
# to their stdin and checks the events on their stdout:
#
#   both say hello and connect, and each sees the other by name; the sharer's app learns the
#   guest's decoders within HELLO_BOUND_S of the guest appearing, unasked; the guest asks (request_control);
#   nothing is granted while the app hasn't answered; the app allows that request; the guest types;
#   the app takes control back (revoke_control); a stale allow is refused; both leave.
#
# The "is controlling" label must be on screen during control and gone after (hard rule 4), and
# `tc-spike replay` checks that what was recorded is what the guest sent.
#
# Then the app dies: the engine's stdin closes and it must exit within two seconds.
#
# Needs `livekit-server --dev` running, jq, swiftc, and a release build:
#   cargo build --release -p twincursor-engine-cli -p twincursor-media-spike
# Usage: bench/engine-e2e.sh   (BIN=path/to/tc-engine SPIKE=path/to/tc-spike to use other builds)
# Nothing needs Screen Recording, a microphone or an unlocked screen.
set -euo pipefail

cd "$(dirname "$0")/.."
BIN=${BIN:-target/release/tc-engine}
SPIKE=${SPIKE:-target/release/tc-spike}
OUT=${OUT:-bench/results/engine-e2e-$(date +%s)}
# How long after the guest appears in the sharer's app its decoders may take to follow.
HELLO_BOUND_S=${HELLO_BOUND_S:-1.0}
mkdir -p "$OUT"
swiftc -O -o "$OUT/windows" bench/control/windows.swift
label_shown() { "$OUT/windows" "$1" | grep -q '"name":"TwinCursor control label"'; }

failed=0
pass() { echo "  ok: $1"; }
fail() { echo "  FAIL: $1"; failed=1; }

# Waits up to $3 seconds (default 20) for a line of events file $1 to match jq filter $2.
wait_for() {
  local file=$1 filter=$2 limit=${3:-20}
  for _ in $(seq $((limit * 10))); do
    if [[ -s $file ]] && jq -e -s "map(select($filter)) | length > 0" "$file" >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.1
  done
  return 1
}
check() { if wait_for "$1" "$2" "${4:-20}"; then pass "$3"; else fail "$3"; fi; }
first() { jq -c -s "map(select($2)) | first" "$1"; }

pids=()
cleanup() {
  for pid in "${pids[@]}"; do kill -INT "$pid" 2>/dev/null || true; done
  exec 3>&- 4>&- 5>&- 2>/dev/null || true
}
trap cleanup EXIT

echo "=== a call, driven over IPC (events in $OUT)"
room="e2e-engine-$$"
mkfifo "$OUT/share.in" "$OUT/view.in"
cat >"$OUT/script.json" <<'EOF'
[{"do":"await_control"},{"do":"type","text":"hello","gap_ms":20},{"do":"await_end"}]
EOF

"$BIN" share --ipc --audio off --room "$room" --name Alex --capture pattern --resolution 720 \
  --allow-control --inject "record:$OUT/actions.jsonl" \
  <"$OUT/share.in" >"$OUT/share.jsonl" 2>"$OUT/share.log" &
sharer=$!
pids+=("$sharer")
exec 3>"$OUT/share.in"
check "$OUT/share.jsonl" '.event == "hello" and .role == "sharer" and .version == 1' "sharer says hello"
check "$OUT/share.jsonl" '.event == "connection" and .state == "connected"' "sharer connected"
# The guest joins a call already under way, as a guest does: its first packets reach the sharer
# before the sharer's room has heard it joined (ARCHITECTURE.md, "Remote control over a data
# channel").
check "$OUT/share.jsonl" '.event == "stats" and .fps > 0' "sharer is sending video"

"$BIN" view --ipc --audio off --room "$room" --name Sam --headless \
  --control-script "$OUT/script.json" --sent-log "$OUT/sent.jsonl" \
  <"$OUT/view.in" >"$OUT/view.jsonl" 2>"$OUT/view.log" &
viewer=$!
pids+=("$viewer")
exec 4>"$OUT/view.in"

check "$OUT/view.jsonl" '.event == "hello" and .role == "guest" and .version == 1' "guest says hello"
check "$OUT/view.jsonl" '.event == "connection" and .state == "connected"' "guest connected"
for side in share view; do
  order=$(head -2 "$OUT/$side.jsonl" | jq -r '.event + (if .state then ":" + .state else "" end)' | paste -sd, -)
  [[ $order == "hello,connection:connecting" ]] && pass "$side: hello first, then connecting" || fail "$side starts with $order"
done
# The guest's hello (its decoders) must reach the sharer's app soon after the guest shows up
# there, before anyone asks for control: the sharer picks a codec from it.
hello=$(python3 - "$OUT/share.jsonl" "$HELLO_BOUND_S" <<'PY'
import json, sys, time
path, bound = sys.argv[1], float(sys.argv[2])
seen = decoders = None
start = time.monotonic()
while time.monotonic() - start < 20 and decoders is None:
    try:
        lines = open(path).read().splitlines()
    except OSError:
        lines = []
    for line in lines:
        try:
            e = json.loads(line)
        except ValueError:
            continue
        sam = [p for p in e.get("participants", []) if e.get("event") == "participants" and p.get("name") == "Sam" and p.get("role") == "guest"]
        if sam and seen is None:
            seen = time.monotonic()
        if sam and {"h264", "h265"} <= set(sam[0].get("decoders", [])):
            decoders = time.monotonic()
            break
    time.sleep(0.02)
if seen is None:
    print("never saw Sam")
elif decoders is None:
    print("saw Sam, but not Sam's decoders within 20 s")
else:
    took = decoders - seen
    print(f"{'ok' if took <= bound else 'late'} {took:.2f} s")
PY
)
case $hello in
  ok*) pass "sharer sees Sam, a guest who decodes H.264 and H.265, ${hello#ok } after Sam appears (bound ${HELLO_BOUND_S} s)" ;;
  late*) fail "Sam's decoders reached the sharer's app ${hello#late } after Sam appeared (bound ${HELLO_BOUND_S} s)" ;;
  *) fail "Sam's decoders: $hello" ;;
esac
check "$OUT/view.jsonl" '.event == "participants" and (.participants | any(.name == "Alex" and .role == "sharer"))' "guest sees Alex, the sharer"

echo '{"cmd":"request_control"}' >&4
check "$OUT/share.jsonl" '.event == "control_asked" and .name == "Sam"' "Sam's request reaches the app"
check "$OUT/view.jsonl" '.event == "control" and .state == "asked"' "guest shows it is asking"
request=$(first "$OUT/share.jsonl" '.event == "control_asked"' | jq -r .request)

# With no answer from the app, nothing is granted.
sleep 2
if jq -e -s 'map(select(.event == "control" and .state == "controlling")) | length == 0' "$OUT/share.jsonl" "$OUT/view.jsonl" >/dev/null \
  && [[ ! -s $OUT/actions.jsonl ]]; then
  pass "no control and nothing injected while the app hasn't answered"
else
  fail "control without an answer"
fi
# An answer to a request that was never made grants nothing.
echo "{\"cmd\":\"allow_control\",\"request\":$((request + 100))}" >&3
check "$OUT/share.jsonl" '.event == "error" and (.message | test("no longer open"))' "an allow for another request is refused"

echo "{\"cmd\":\"allow_control\",\"request\":$request}" >&3
check "$OUT/share.jsonl" '.event == "control" and .state == "controlling" and .by.name == "Sam"' "sharer: Sam is controlling"
check "$OUT/view.jsonl" '.event == "control" and .state == "controlling"' "guest: in control"
label_during=0
for _ in $(seq 100); do label_shown "$sharer" && { label_during=1; break; }; sleep 0.05; done
[[ $label_during -eq 1 ]] && pass "the control label is on screen" || fail "no control label during control"

# Wait for the typing to arrive: 5 characters.
for _ in $(seq 100); do
  [[ -s $OUT/actions.jsonl ]] && [[ $(grep -c '"text"' "$OUT/actions.jsonl" || true) -ge 5 ]] && break
  sleep 0.1
done
sleep 0.5
echo '{"cmd":"revoke_control"}' >&3
check "$OUT/share.jsonl" '.event == "control" and .state == "none"' "sharer: control taken back"
check "$OUT/view.jsonl" '.event == "control" and .state == "none"' "guest: no longer in control"
sleep 0.5
label_shown "$sharer" && fail "the control label is still up after control" || pass "the control label is gone"
echo "{\"cmd\":\"allow_control\",\"request\":$request}" >&3
sleep 0.5
[[ $(jq -s 'map(select(.event == "error" and (.message | test("no longer open")))) | length' "$OUT/share.jsonl") -ge 2 ]] \
  && pass "a stale allow after the revoke is refused" || fail "stale allow not refused"
if "$SPIKE" replay --sent "$OUT/sent.jsonl" --actions "$OUT/actions.jsonl" >"$OUT/replay.log" 2>&1; then
  pass "what was recorded is what the guest sent ($(tail -1 "$OUT/replay.log"))"
else
  fail "replay: $(tail -3 "$OUT/replay.log")"
fi

echo '{"cmd":"leave"}' >&4
check "$OUT/view.jsonl" '.event == "ended" and .reason == "left"' "guest left"
check "$OUT/share.jsonl" '.event == "participants" and (.participants | length == 0)' "sharer sees the guest go"
echo '{"cmd":"leave"}' >&3
check "$OUT/share.jsonl" '.event == "ended" and .reason == "left"' "sharer left"
for pid in "$sharer" "$viewer"; do
  for _ in $(seq 50); do kill -0 "$pid" 2>/dev/null || break; sleep 0.1; done
  kill -0 "$pid" 2>/dev/null && fail "process $pid still running after leave" || pass "process $pid exited"
done
[[ $(tail -1 "$OUT/share.jsonl" | jq -r .event) == ended ]] && pass "ended is the sharer's last line" || fail "ended isn't last"
bad=$(cat "$OUT/share.jsonl" "$OUT/view.jsonl" | jq -R 'fromjson? // "bad"' | grep -c '^"bad"$' || true)
[[ $bad -eq 0 ]] && pass "stdout carries nothing but JSON events" || fail "$bad stdout lines aren't JSON"
exec 3>&- 4>&-

echo "=== the app dies: the engine must exit"
for side in share view; do
  mkfifo "$OUT/die-$side.in"
  if [[ $side == share ]]; then
    "$BIN" share --ipc --audio off --room "$room-die" --name Alex --capture pattern --resolution 720 \
      --allow-control --inject log <"$OUT/die-$side.in" >"$OUT/die-$side.jsonl" 2>"$OUT/die-$side.log" &
  else
    "$BIN" view --ipc --audio off --room "$room-die" --name Sam --headless \
      <"$OUT/die-$side.in" >"$OUT/die-$side.jsonl" 2>"$OUT/die-$side.log" &
  fi
  pid=$!
  pids+=("$pid")
  exec 5>"$OUT/die-$side.in"
  check "$OUT/die-$side.jsonl" '.event == "connection" and .state == "connected"' "$side connected"
  start=$(python3 -c 'import time; print(time.time())')
  exec 5>&-
  for _ in $(seq 50); do kill -0 "$pid" 2>/dev/null || break; sleep 0.02; done
  end=$(python3 -c 'import time; print(time.time())')
  ms=$(python3 -c "print(round(($end - $start) * 1000))")
  if kill -0 "$pid" 2>/dev/null; then
    fail "$side still running ${ms} ms after its stdin closed"
    kill -INT "$pid" 2>/dev/null || true
  elif [[ $ms -le 2000 ]]; then
    pass "$side exited ${ms} ms after its stdin closed"
  else
    fail "$side took ${ms} ms to exit"
  fi
done

if [[ $failed -eq 0 ]]; then echo "engine-e2e: PASS"; else echo "engine-e2e: FAIL (events and logs in $OUT)"; fi
exit $failed
