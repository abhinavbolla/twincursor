#!/usr/bin/env bash
# The desktop app end to end, with nobody at the Mac.
#
# Starts the real app (`twincursor-desktop`) in its test-only script mode (`--script PATH
# --report PATH`): it shares the synthetic pattern with voice off and recorded input (nothing is
# injected, nothing makes a sound), answers control requests as the script says, and writes
# everything it sees to the report. A headless `tc-engine view` plays the guest.
#
# Case 1, consent: the guest appears in the app with its name and decoders; its first request
# reaches the app and is denied, and nothing is injected; its second is allowed, it types, and
# what it typed is recorded; the app takes control back; the app leaves and its engine exits.
# Case 2, HEVC: the app shares at 4K with HEVC and a guest joins that announces no HEVC; the app
# restarts its engine with H.264, the guest gets video again in H.264, and when the app is
# killed, its engine goes with it.
#
# Needs `livekit-server --dev` running, jq, and release builds:
#   cargo build --release -p twincursor-desktop -p twincursor-engine-cli -p twincursor-media-spike
# (the UI first: `npm ci && npm run build` in apps/desktop).
# Usage: bench/app-e2e.sh   (TARGET=dir to use another target directory)
set -euo pipefail

cd "$(dirname "$0")/.."
TARGET=${TARGET:-${CARGO_TARGET_DIR:-target}}
APP=${APP:-$TARGET/release/twincursor-desktop}
BIN=${BIN:-$TARGET/release/tc-engine}
SPIKE=${SPIKE:-$TARGET/release/tc-spike}
OUT=${OUT:-bench/results/app-e2e-$(date +%s)}
mkdir -p "$OUT"
OUT=$(cd "$OUT" && pwd)

failed=0
pass() { echo "  ok: $1"; }
fail() { echo "  FAIL: $1"; failed=1; }

# Waits up to $3 seconds (default 30) for a line of JSON-lines file $1 to match jq filter $2.
wait_for() {
  local file=$1 filter=$2 limit=${3:-30}
  for _ in $(seq $((limit * 10))); do
    if [[ -s $file ]] && jq -e -s "map(select($filter)) | length > 0" "$file" >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.1
  done
  return 1
}
check() { if wait_for "$1" "$2" "${4:-30}"; then pass "$3"; else fail "$3"; fi; }
count() { jq -s "map(select($2)) | length" "$1"; }
# An event the app saw, as the report has it.
ev() { echo ".kind == \"event\" and ($1)"; }
running() { kill -0 "$1" 2>/dev/null; }

pids=()
cleanup() {
  for pid in "${pids[@]}"; do kill -INT "$pid" 2>/dev/null || true; done
  exec 4>&- 2>/dev/null || true
}
trap cleanup EXIT

start_guest() { # name, extra flags...
  local name=$1
  shift
  mkfifo "$OUT/$name.in"
  "$BIN" view --ipc --audio off --room "$room" --name Sam --headless "$@" \
    <"$OUT/$name.in" >"$OUT/$name.jsonl" 2>"$OUT/$name.log" &
  guest=$!
  pids+=("$guest")
  exec 4>"$OUT/$name.in"
}

echo "=== case 1: consent, through the app (results in $OUT)"
room="e2e-app-$$"
cat >"$OUT/consent.json" <<EOF
{
  "call": {"room": "$room", "name": "Alex", "quality": "1080p", "inject_record": "$OUT/actions.jsonl"},
  "steps": [
    {"do": "wait_for", "event": {"event": "connection", "state": "connected"}},
    {"do": "answer", "with": "deny"},
    {"do": "answer", "with": "allow"},
    {"do": "wait_for", "event": {"event": "control", "state": "controlling"}},
    {"do": "wait_ms", "ms": 2000},
    {"do": "send", "cmd": "revoke_control"},
    {"do": "wait_for", "event": {"event": "control", "state": "none"}},
    {"do": "wait_ms", "ms": 500},
    {"do": "send", "cmd": "leave"},
    {"do": "wait_exit"}
  ],
  "quit": true
}
EOF
cat >"$OUT/guest-script.json" <<'EOF'
[{"do":"await_control"},{"do":"type","text":"hello","gap_ms":20},{"do":"await_end"}]
EOF
"$APP" --script "$OUT/consent.json" --report "$OUT/consent.jsonl" 2>"$OUT/consent-app.log" &
app=$!
pids+=("$app")
check "$OUT/consent.jsonl" '.kind == "started" and .role == "share"' "the app started its engine"
started=$(jq -c -s 'map(select(.kind == "started")) | first' "$OUT/consent.jsonl")
args=$(jq -r '.args | join(" ")' <<<"$started")
for flag in "--allow-control" "--exclude-app-pid=$app" "--audio=off" "--capture=pattern" \
  "--inject=record:$OUT/actions.jsonl" "--codec=h264" "--resolution=1080"; do
  [[ " $args " == *" $flag "* ]] && pass "engine flag $flag" || fail "engine flags lack $flag: $args"
done
[[ $args != *secret* ]] && pass "no secret among the engine's flags" || fail "a secret in the flags: $args"
check "$OUT/consent.jsonl" "$(ev '.event.event == "connection" and .event.state == "connected"')" "the app's engine connected"

start_guest consent-guest --control-script "$OUT/guest-script.json" --sent-log "$OUT/sent.jsonl"
check "$OUT/consent.jsonl" "$(ev '.event.event == "participants" and (.event.participants | any(.name == "Sam" and .role == "guest" and (.decoders | any(. == "h264"))))')" \
  "the app sees Sam, a guest, with decoders"

echo '{"cmd":"request_control"}' >&4
check "$OUT/consent.jsonl" "$(ev '.event.event == "control_asked" and .event.name == "Sam"')" "Sam's first request reaches the app"
check "$OUT/consent.jsonl" '.kind == "script" and .step == 1 and .ok' "the script denied it"
check "$OUT/consent-guest.jsonl" '.event == "control" and .state == "none"' "the guest is told no"
sleep 2
if [[ ! -s $OUT/actions.jsonl ]] && [[ $(count "$OUT/consent.jsonl" "$(ev '.event.event == "control" and .event.state == "controlling"')") -eq 0 ]]; then
  pass "after a deny, no control and nothing injected"
else
  fail "control or input after a deny"
fi

echo '{"cmd":"request_control"}' >&4
check "$OUT/consent.jsonl" "$(ev '.event.event == "control_asked" and .event.request == 2')" "Sam's second request reaches the app"
check "$OUT/consent.jsonl" "$(ev '.event.event == "control" and .event.state == "controlling" and .event.by.name == "Sam"')" "allowed: the app shows Sam controlling"
check "$OUT/consent-guest.jsonl" '.event == "control" and .state == "controlling"' "the guest is in control"
for _ in $(seq 100); do
  [[ -s $OUT/actions.jsonl ]] && [[ $(grep -c '"text"' "$OUT/actions.jsonl" || true) -ge 5 ]] && break
  sleep 0.1
done
check "$OUT/consent.jsonl" '.kind == "script" and .step == 6 and .ok' "the app took control back"
guest_free=0
for _ in $(seq 100); do
  jq -e -s 'map(select(.event == "control")) | last | .state == "none"' "$OUT/consent-guest.jsonl" >/dev/null 2>&1 && { guest_free=1; break; }
  sleep 0.1
done
[[ $guest_free -eq 1 ]] && pass "the guest is no longer in control" || fail "the guest still has control"
if "$SPIKE" replay --sent "$OUT/sent.jsonl" --actions "$OUT/actions.jsonl" >"$OUT/replay.log" 2>&1; then
  pass "what was recorded is what the guest sent ($(tail -1 "$OUT/replay.log"))"
else
  fail "replay: $(tail -3 "$OUT/replay.log")"
fi

check "$OUT/consent.jsonl" "$(ev '.event.event == "ended" and .event.reason == "left"')" "the app left the call"
check "$OUT/consent.jsonl" '.kind == "exited" and .code == 0' "the app's engine exited"
engine=$(jq -r '.pid' <<<"$started")
running "$engine" && fail "engine $engine still running after leave" || pass "engine $engine is gone"
for _ in $(seq 50); do running "$app" || break; sleep 0.1; done
running "$app" && fail "the app didn't quit after its script" || pass "the app quit after its script"
check "$OUT/consent.jsonl" '.kind == "script" and .done and .ok' "every script step passed"
echo '{"cmd":"leave"}' >&4
check "$OUT/consent-guest.jsonl" '.event == "ended" and .reason == "left"' "the guest left"
exec 4>&-

echo "=== case 2: a guest without HEVC, then the app is killed"
room="e2e-app-hevc-$$"
cat >"$OUT/hevc.json" <<EOF
{
  "call": {"room": "$room", "name": "Alex", "quality": "4k", "inject_record": "$OUT/hevc-actions.jsonl"},
  "steps": [],
  "quit": false
}
EOF
"$APP" --script "$OUT/hevc.json" --report "$OUT/hevc.jsonl" 2>"$OUT/hevc-app.log" &
app=$!
pids+=("$app")
check "$OUT/hevc.jsonl" '.kind == "started" and .codec == "h265"' "the app shares at 4K with HEVC"
check "$OUT/hevc.jsonl" "$(ev '.event.event == "stats" and .event.codec == "h265"')" "HEVC video is going out"
start_guest hevc-guest --decoders h264,vp8,vp9,av1
check "$OUT/hevc.jsonl" "$(ev '.event.event == "participants" and (.event.participants | any(.name == "Sam" and (.decoders | length > 0) and (.decoders | any(. == "h265") | not)))')" \
  "the app sees a guest without HEVC"
check "$OUT/hevc.jsonl" '.kind == "restarting" and .codec == "h264" and (.message | test("^Sam.s computer can.t decode HEVC; switched to H.264"))' "the app says it switched to H.264"
check "$OUT/hevc.jsonl" '.kind == "started" and .codec == "h264" and any(.args[]; . == "--codec=h264") and any(.args[]; . == "--resolution=2160")' "a new engine, same room and resolution, with H.264"
first=$(jq -r -s 'map(select(.kind == "started")) | first | .pid' "$OUT/hevc.jsonl")
second=$(jq -r -s 'map(select(.kind == "started")) | last | .pid' "$OUT/hevc.jsonl")
running "$first" && fail "the HEVC engine $first is still running" || pass "the HEVC engine is gone"
check "$OUT/hevc-guest.jsonl" '.event == "stats" and .codec == "h264" and .fps > 0' "the guest gets video again, in H.264" 40
sleep 3
[[ $(count "$OUT/hevc.jsonl" '.kind == "restarting"') -eq 1 ]] && pass "it switched once" || fail "it switched more than once"

kill -9 "$app"
wait "$app" 2>/dev/null || true
for _ in $(seq 40); do running "$second" || break; sleep 0.05; done
running "$second" && fail "engine $second outlived the killed app" || pass "killing the app took its engine with it"
echo '{"cmd":"leave"}' >&4
check "$OUT/hevc-guest.jsonl" '.event == "ended" and .reason == "left"' "the guest left"
exec 4>&-

if [[ $failed -eq 0 ]]; then echo "app-e2e: PASS"; else echo "app-e2e: FAIL (reports and logs in $OUT)"; fi
exit $failed
