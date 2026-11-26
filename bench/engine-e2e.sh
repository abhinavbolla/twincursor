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
# Then the same call with call passes (`tc-engine dev-pass`, in TWINCURSOR_PASS) and no --room or
# --name: each side sees the other under the pass's identity, name and role, video flows, control
# is asked for, allowed, used and revoked, and neither pass shows up in any event or log. A
# guest's pass given to `share`, and a sharer's to `view`, must fail before connecting.
#
# Then trusted control, with passes (docs/adr/0010-trusted-control.md): after `trust_control` the
# guest's requests are granted with no `control_asked`, again after giving control back; a revoke,
# before or after the guest asks, takes the trust back, so the next request is a prompt; trusting
# someone not in the call, or the sharer, is an error; a guest who leaves and comes back with the
# same pass is asked again; a share without --allow-control refuses both trust and requests. The
# guest claims another name in its hello (`--hello-name`): the sharer must show the pass's.
# Then trust on arrival: `trust_on_arrival` for someone not in the call yet, then a revoke, then
# they join and ask: a prompt; and for someone else, who joins and is granted with no prompt.
#
# Needs `livekit-server --dev` running, jq, swiftc, and a release build:
#   cargo build --release -p twincursor-engine-cli -p twincursor-media-spike
# Usage: bench/engine-e2e.sh   (BIN=path/to/tc-engine SPIKE=path/to/tc-spike to use other builds)
# Last, one window shared with --allow-control: every request is turned down with the reason
# both sides see, every trust is refused with it, and nothing is injected. That part captures a
# real window (headless: nothing opens) and is skipped without Screen Recording. Nothing else
# needs Screen Recording, a microphone or an unlocked screen.
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

echo "=== a call with call passes: no --room, no --name, no key or secret"
# Mints passes as the account server would (dev-pass signs with the dev server's key), and hands
# them over as the app does, in TWINCURSOR_PASS.
proom="e2e-pass-$$"
sid=$(uuidgen | tr '[:upper:]' '[:lower:]')
gid=$(uuidgen | tr '[:upper:]' '[:lower:]')
sharer_pass=$("$BIN" dev-pass --room "$proom" --role sharer --name "Alex Account" --id "$sid" --ttl 600)
guest_pass=$("$BIN" dev-pass --room "$proom" --role guest --name "Sam Account" --id "$gid" --ttl 600)
mkfifo "$OUT/pass-share.in" "$OUT/pass-view.in"

TWINCURSOR_PASS=$sharer_pass "$BIN" share --ipc --audio off --capture pattern --resolution 720 \
  --allow-control --inject "record:$OUT/pass-actions.jsonl" \
  <"$OUT/pass-share.in" >"$OUT/pass-share.jsonl" 2>"$OUT/pass-share.log" &
psharer=$!
pids+=("$psharer")
exec 3>"$OUT/pass-share.in"
check "$OUT/pass-share.jsonl" '.event == "connection" and .state == "connected"' "sharer connected with its pass"
check "$OUT/pass-share.jsonl" '.event == "stats" and .fps > 0' "sharer is sending video"

TWINCURSOR_PASS=$guest_pass "$BIN" view --ipc --audio off --headless \
  --control-script "$OUT/script.json" --sent-log "$OUT/pass-sent.jsonl" \
  <"$OUT/pass-view.in" >"$OUT/pass-view.jsonl" 2>"$OUT/pass-view.log" &
pviewer=$!
pids+=("$pviewer")
exec 4>"$OUT/pass-view.in"
check "$OUT/pass-view.jsonl" '.event == "connection" and .state == "connected"' "guest connected with its pass"
check "$OUT/pass-share.jsonl" ".event == \"participants\" and (.participants | any(.identity == \"guest-$gid\" and .name == \"Sam Account\" and .role == \"guest\"))" \
  "sharer sees guest-<uuid> as Sam Account, a guest"
check "$OUT/pass-view.jsonl" ".event == \"participants\" and (.participants | any(.identity == \"sharer-$sid\" and .name == \"Alex Account\" and .role == \"sharer\"))" \
  "guest sees sharer-<uuid> as Alex Account, the sharer"
check "$OUT/pass-view.jsonl" '.event == "stats" and .fps > 0' "guest is receiving video"

echo '{"cmd":"request_control"}' >&4
check "$OUT/pass-share.jsonl" ".event == \"control_asked\" and .name == \"Sam Account\" and .identity == \"guest-$gid\"" "Sam Account's request reaches the app"
request=$(first "$OUT/pass-share.jsonl" '.event == "control_asked"' | jq -r .request)
echo "{\"cmd\":\"allow_control\",\"request\":$request}" >&3
check "$OUT/pass-share.jsonl" '.event == "control" and .state == "controlling" and .by.name == "Sam Account"' "sharer: Sam Account is controlling"
check "$OUT/pass-view.jsonl" '.event == "control" and .state == "controlling"' "guest: in control"
label_during=0
for _ in $(seq 100); do label_shown "$psharer" && { label_during=1; break; }; sleep 0.05; done
[[ $label_during -eq 1 ]] && pass "the control label is on screen" || fail "no control label during control"
for _ in $(seq 100); do
  [[ -s $OUT/pass-actions.jsonl ]] && [[ $(grep -c '"text"' "$OUT/pass-actions.jsonl" || true) -ge 5 ]] && break
  sleep 0.1
done
sleep 0.5
echo '{"cmd":"revoke_control"}' >&3
check "$OUT/pass-share.jsonl" '.event == "control" and .state == "none"' "sharer: control taken back"
check "$OUT/pass-view.jsonl" '.event == "control" and .state == "none"' "guest: no longer in control"
if "$SPIKE" replay --sent "$OUT/pass-sent.jsonl" --actions "$OUT/pass-actions.jsonl" >"$OUT/pass-replay.log" 2>&1; then
  pass "what was recorded is what the guest sent ($(tail -1 "$OUT/pass-replay.log"))"
else
  fail "replay: $(tail -3 "$OUT/pass-replay.log")"
fi
echo '{"cmd":"leave"}' >&4
check "$OUT/pass-view.jsonl" '.event == "ended" and .reason == "left"' "guest left"
echo '{"cmd":"leave"}' >&3
check "$OUT/pass-share.jsonl" '.event == "ended" and .reason == "left"' "sharer left"
exec 3>&- 4>&-
for pid in "$psharer" "$pviewer"; do
  for _ in $(seq 50); do kill -0 "$pid" 2>/dev/null || break; sleep 0.1; done
  kill -0 "$pid" 2>/dev/null && fail "process $pid still running after leave" || pass "process $pid exited"
done
leaked=0
for pass_value in "$sharer_pass" "$guest_pass"; do
  for part in $(tr '.' ' ' <<<"$pass_value"); do
    grep -qF -- "$part" "$OUT"/pass-*.jsonl "$OUT"/pass-*.log 2>/dev/null && leaked=1
  done
done
[[ $leaked -eq 0 ]] && pass "neither pass appears in any event or log" || fail "a pass leaked into events or logs"

echo "=== trusted control, with call passes"
# How many lines of events file $1 match jq filter $2.
count() { jq -s "map(select($2)) | length" "$1"; }
# Waits up to 20 s for at least $3 lines of $1 to match $2.
wait_count() {
  for _ in $(seq 200); do [[ $(count "$1" "$2") -ge $3 ]] && return 0; sleep 0.1; done
  return 1
}
texts() { if [[ -s $1 ]]; then grep -c '"text"' "$1" || true; else echo 0; fi; }
troom="e2e-trust-$$"
tsid=$(uuidgen | tr '[:upper:]' '[:lower:]')
tgid=$(uuidgen | tr '[:upper:]' '[:lower:]')
tsharer_pass=$("$BIN" dev-pass --room "$troom" --role sharer --name "Alex Account" --id "$tsid" --ttl 600)
tguest_pass=$("$BIN" dev-pass --room "$troom" --role guest --name "Sam Account" --id "$tgid" --ttl 600)
guest="guest-$tgid"
TS=$OUT/trust-share.jsonl
TV=$OUT/trust-view.jsonl
mkfifo "$OUT/trust-share.in" "$OUT/trust-view.in"
# Two grants with typing, then stay in the call until told to leave.
cat >"$OUT/trust-script.json" <<'EOF'
[{"do":"await_control"},{"do":"type","text":"hello","gap_ms":20},{"do":"await_end"},
 {"do":"await_control"},{"do":"type","text":"again","gap_ms":20},{"do":"await_end"},
 {"do":"wait","ms":600000}]
EOF

TWINCURSOR_PASS=$tsharer_pass "$BIN" share --ipc --audio off --capture pattern --resolution 720 \
  --allow-control --inject "record:$OUT/trust-actions.jsonl" \
  <"$OUT/trust-share.in" >"$TS" 2>"$OUT/trust-share.log" &
tsharer=$!
pids+=("$tsharer")
exec 3>"$OUT/trust-share.in"
check "$TS" '.event == "connection" and .state == "connected"' "sharer connected with its pass"
# The guest's own app claims to be Mallory; its pass says Sam Account.
TWINCURSOR_PASS=$tguest_pass "$BIN" view --ipc --audio off --headless --hello-name Mallory \
  --control-script "$OUT/trust-script.json" --sent-log "$OUT/trust-sent.jsonl" \
  <"$OUT/trust-view.in" >"$TV" 2>"$OUT/trust-view.log" &
tviewer=$!
pids+=("$tviewer")
exec 4>"$OUT/trust-view.in"
check "$TV" '.event == "connection" and .state == "connected"' "guest connected with its pass"
check "$TS" ".event == \"participants\" and (.participants | any(.identity == \"$guest\" and .name == \"Sam Account\"))" \
  "sharer sees Sam Account"
check "$TV" '.event == "stats" and .fps > 0' "guest is receiving video"

trusted_sam=".event == \"control\" and (.trusted | length == 1 and .[0].identity == \"$guest\" and .[0].name == \"Sam Account\")"
echo "{\"cmd\":\"trust_control\",\"identity\":\"$guest\"}" >&3
check "$TS" "$trusted_sam and .state == \"none\"" "trust_control: the control event lists Sam Account as trusted"
echo '{"cmd":"request_control"}' >&4
check "$TS" '.event == "control" and .state == "controlling" and .by.name == "Sam Account"' "Sam Account's request is granted at once, under the pass's name"
check "$TV" '.event == "control" and .state == "controlling"' "guest: in control"
label_during=0
for _ in $(seq 100); do label_shown "$tsharer" && { label_during=1; break; }; sleep 0.05; done
[[ $label_during -eq 1 ]] && pass "the control label is on screen" || fail "no control label during trusted control"
for _ in $(seq 100); do [[ $(texts "$OUT/trust-actions.jsonl") -ge 5 ]] && break; sleep 0.1; done
[[ $(texts "$OUT/trust-actions.jsonl") -ge 5 ]] && pass "the guest's typing is recorded" || fail "typing not recorded"
[[ $(count "$TS" '.event == "control_asked"') -eq 0 ]] && pass "no control_asked for a trusted guest" || fail "a trusted request raised a prompt"
[[ $(count "$TS" '.event == "control" and .state == "asked"') -eq 0 ]] && pass "never in the asked state" || fail "a trusted request was shown as asking"

sleep 0.5
echo '{"cmd":"release_control"}' >&4
if wait_count "$TS" "$trusted_sam and .state == \"none\"" 2; then pass "the guest gives control back and is still trusted"; else fail "release lost the trust"; fi
if wait_count "$TV" '.event == "control" and .state == "none"' 2; then pass "guest: no longer in control"; else fail "guest still in control"; fi
sleep 0.3
echo '{"cmd":"request_control"}' >&4
if wait_count "$TS" '.event == "control" and .state == "controlling"' 2; then pass "asking again is granted again"; else fail "second trusted request not granted"; fi
for _ in $(seq 100); do [[ $(texts "$OUT/trust-actions.jsonl") -ge 10 ]] && break; sleep 0.1; done
[[ $(count "$TS" '.event == "control_asked"') -eq 0 ]] && pass "still no control_asked" || fail "the second trusted request raised a prompt"
sleep 0.5

echo '{"cmd":"revoke_control"}' >&3
check "$TS" '.event == "control" and .state == "none" and (.trusted | length == 0)' "revoke_control: control ends and nobody is trusted"
if wait_count "$TV" '.event == "control" and .state == "none"' 3; then pass "guest: no longer in control"; else fail "guest still in control after the revoke"; fi
if "$SPIKE" replay --sent "$OUT/trust-sent.jsonl" --actions "$OUT/trust-actions.jsonl" >"$OUT/trust-replay.log" 2>&1; then
  pass "what was recorded is what the guest sent, both grants ($(tail -1 "$OUT/trust-replay.log"))"
else
  fail "replay: $(tail -3 "$OUT/trust-replay.log")"
fi
sleep 0.3
echo '{"cmd":"request_control"}' >&4
if wait_count "$TS" ".event == \"control_asked\" and .identity == \"$guest\" and .name == \"Sam Account\"" 1; then
  pass "after the revoke, the next request asks the app (as Sam Account, not Mallory)"
else
  fail "no control_asked after the revoke"
fi
request=$(first "$TS" '.event == "control_asked"' | jq -r .request)
echo "{\"cmd\":\"deny_control\",\"request\":$request}" >&3
if wait_count "$TV" '.event == "control" and .state == "none"' 4; then pass "denied by its number"; else fail "deny after revoke"; fi
sleep 0.5

trusts=$(count "$TS" "$trusted_sam")
echo "{\"cmd\":\"trust_control\",\"identity\":\"$guest\"}" >&3
if wait_count "$TS" "$trusted_sam" $((trusts + 1)); then pass "trusted again"; else fail "second trust_control"; fi
untrusts=$(count "$TS" '.event == "control" and .state == "none" and (.trusted | length == 0)')
echo '{"cmd":"revoke_control"}' >&3
if wait_count "$TS" '.event == "control" and .state == "none" and (.trusted | length == 0)' $((untrusts + 1)); then
  pass "a revoke before the guest asks takes the trust back"
else
  fail "revoke while idle left the trust"
fi
echo '{"cmd":"request_control"}' >&4
if wait_count "$TS" '.event == "control_asked"' 2; then pass "so the request asks the app"; else fail "no prompt after an idle revoke"; fi
request=$(jq -c -s 'map(select(.event == "control_asked")) | last' "$TS" | jq -r .request)
[[ $(count "$TS" '.event == "control" and .state == "controlling"') -eq 2 ]] && pass "and nothing was granted" || fail "granted after an idle revoke"
echo "{\"cmd\":\"deny_control\",\"request\":$request}" >&3
wait_count "$TV" '.event == "control" and .state == "none"' 5 || true
sleep 0.3

errors_before=$(count "$TS" '.event == "error"')
trusts=$(count "$TS" "$trusted_sam")
echo '{"cmd":"trust_control","identity":"guest-not-in-this-call"}' >&3
check "$TS" ".event == \"error\" and .message == \"That person isn't in the call.\"" "trusting someone not in the call is an error"
echo "{\"cmd\":\"trust_control\",\"identity\":\"sharer-$tsid\"}" >&3
check "$TS" '.event == "error" and .message == "Only a guest can be given control."' "trusting the sharer itself is an error"
sleep 0.3
[[ $(count "$TS" '.event == "error"') -eq $((errors_before + 2)) && $(count "$TS" "$trusted_sam") -eq $trusts \
  && $(count "$TS" '.event == "control" and (.trusted | any(.identity != "'"$guest"'"))') -eq 0 ]] \
  && pass "neither trusted anyone" || fail "a refused trust changed something"

echo "{\"cmd\":\"trust_control\",\"identity\":\"$guest\"}" >&3
if wait_count "$TS" "$trusted_sam" $((trusts + 1)); then pass "trusted once more"; else fail "third trust_control"; fi
untrusts=$(count "$TS" '.event == "control" and .state == "none" and (.trusted | length == 0)')
echo '{"cmd":"leave"}' >&4
exec 4>&-
check "$TS" '.event == "participants" and (.participants | length == 0)' "the guest leaves"
if wait_count "$TS" '.event == "control" and .state == "none" and (.trusted | length == 0)' $((untrusts + 1)); then
  pass "leaving takes the trust away"
else
  fail "the trust outlived the guest"
fi
for _ in $(seq 50); do kill -0 "$tviewer" 2>/dev/null || break; sleep 0.1; done
# The same pass, so the same identity, back in the call.
mkfifo "$OUT/trust-back.in"
TWINCURSOR_PASS=$tguest_pass "$BIN" view --ipc --audio off --headless \
  <"$OUT/trust-back.in" >"$OUT/trust-back.jsonl" 2>"$OUT/trust-back.log" &
tback=$!
pids+=("$tback")
exec 4>"$OUT/trust-back.in"
check "$OUT/trust-back.jsonl" '.event == "connection" and .state == "connected"' "the same pass joins again"
if wait_count "$TS" ".event == \"participants\" and (.participants | any(.identity == \"$guest\"))" 2; then pass "the sharer sees $guest again"; else fail "rejoin not seen"; fi
sleep 0.5
echo '{"cmd":"request_control"}' >&4
if wait_count "$TS" ".event == \"control_asked\" and .identity == \"$guest\"" 3; then
  pass "the returning guest's request asks the app"
else
  fail "the returning guest was not asked about"
fi
[[ $(count "$TS" '.event == "control" and .state == "controlling"') -eq 2 ]] && pass "and isn't granted" || fail "the returning guest got control"
request=$(jq -c -s 'map(select(.event == "control_asked")) | last' "$TS" | jq -r .request)
echo "{\"cmd\":\"deny_control\",\"request\":$request}" >&3
if wait_count "$OUT/trust-back.jsonl" '.event == "control" and .state == "none"' 2; then pass "denied"; else fail "deny for the returning guest"; fi
echo '{"cmd":"leave"}' >&4
echo '{"cmd":"leave"}' >&3
check "$TS" '.event == "ended" and .reason == "left"' "sharer left"
exec 3>&- 4>&-
for pid in "$tsharer" "$tviewer" "$tback"; do
  for _ in $(seq 50); do kill -0 "$pid" 2>/dev/null || break; sleep 0.1; done
  kill -0 "$pid" 2>/dev/null && fail "process $pid still running after leave" || pass "process $pid exited"
done
[[ $(count "$TS" '.event == "control_asked" and .name == "Mallory"') -eq 0 \
  && $(count "$TS" '.event == "control" and (.by.name == "Mallory" or (.trusted | any(.name == "Mallory")))') -eq 0 ]] \
  && pass "the name in the guest's hello (Mallory) is never shown" || fail "the hello's name was shown"

echo "=== trust on arrival, with call passes"
# "Let in with control" for someone not in the call yet: trusted when they join, unless any
# revoke came first.
aroom="e2e-arrive-$$"
asid=$(uuidgen | tr '[:upper:]' '[:lower:]')
aaid=$(uuidgen | tr '[:upper:]' '[:lower:]')
abid=$(uuidgen | tr '[:upper:]' '[:lower:]')
asharer_pass=$("$BIN" dev-pass --room "$aroom" --role sharer --name "Alex Account" --id "$asid" --ttl 600)
aa_pass=$("$BIN" dev-pass --room "$aroom" --role guest --name "Ana Account" --id "$aaid" --ttl 600)
ab_pass=$("$BIN" dev-pass --room "$aroom" --role guest --name "Bo Account" --id "$abid" --ttl 600)
AS=$OUT/arrive-share.jsonl
mkfifo "$OUT/arrive-share.in" "$OUT/arrive-a.in" "$OUT/arrive-b.in"
TWINCURSOR_PASS=$asharer_pass "$BIN" share --ipc --audio off --capture pattern --resolution 720 \
  --allow-control --inject "record:$OUT/arrive-actions.jsonl" \
  <"$OUT/arrive-share.in" >"$AS" 2>"$OUT/arrive-share.log" &
asharer=$!
pids+=("$asharer")
exec 3>"$OUT/arrive-share.in"
check "$AS" '.event == "connection" and .state == "connected"' "sharer connected with its pass"

echo "{\"cmd\":\"trust_on_arrival\",\"identity\":\"guest-$aaid\"}" >&3
sleep 0.3
echo '{"cmd":"revoke_control"}' >&3
sleep 0.3
TWINCURSOR_PASS=$aa_pass "$BIN" view --ipc --audio off --headless \
  <"$OUT/arrive-a.in" >"$OUT/arrive-a.jsonl" 2>"$OUT/arrive-a.log" &
aviewer=$!
pids+=("$aviewer")
exec 4>"$OUT/arrive-a.in"
check "$AS" ".event == \"participants\" and (.participants | any(.identity == \"guest-$aaid\"))" "trust_on_arrival, then revoke_control, then Ana joins"
sleep 0.5
echo '{"cmd":"request_control"}' >&4
check "$AS" ".event == \"control_asked\" and .identity == \"guest-$aaid\"" "Ana's request asks the app: the revoke cancelled it"
[[ $(count "$AS" '.event == "control" and (.state == "controlling" or (.trusted | length > 0))') -eq 0 ]] \
  && pass "Ana was never trusted or given control" || fail "Ana got control after a revoke"
request=$(first "$AS" '.event == "control_asked"' | jq -r .request)
echo "{\"cmd\":\"deny_control\",\"request\":$request}" >&3
if wait_count "$OUT/arrive-a.jsonl" '.event == "control" and .state == "none"' 2; then pass "denied"; else fail "deny for Ana"; fi
echo '{"cmd":"leave"}' >&4
exec 4>&-

echo "{\"cmd\":\"trust_on_arrival\",\"identity\":\"guest-$abid\"}" >&3
sleep 0.3
[[ $(count "$AS" '.event == "error"') -eq 0 ]] && pass "trust_on_arrival for someone not in the call yet is taken" || fail "trust_on_arrival was refused: $(first "$AS" '.event == "error"')"
TWINCURSOR_PASS=$ab_pass "$BIN" view --ipc --audio off --headless \
  <"$OUT/arrive-b.in" >"$OUT/arrive-b.jsonl" 2>"$OUT/arrive-b.log" &
bviewer=$!
pids+=("$bviewer")
exec 4>"$OUT/arrive-b.in"
check "$AS" ".event == \"control\" and (.trusted | any(.identity == \"guest-$abid\" and .name == \"Bo Account\"))" "Bo joins and is trusted on arrival"
sleep 0.5
echo '{"cmd":"request_control"}' >&4
check "$AS" '.event == "control" and .state == "controlling" and .by.name == "Bo Account"' "Bo's request is granted at once"
[[ $(count "$AS" ".event == \"control_asked\" and .identity == \"guest-$abid\"") -eq 0 ]] && pass "with no control_asked" || fail "Bo's request raised a prompt"
echo '{"cmd":"revoke_control"}' >&3
check "$AS" '.event == "control" and .state == "none" and (.trusted | length == 0) and .by == null' "revoke_control takes it back"
echo '{"cmd":"leave"}' >&4
echo '{"cmd":"leave"}' >&3
check "$AS" '.event == "ended" and .reason == "left"' "sharer left"
exec 3>&- 4>&-
for pid in "$asharer" "$aviewer" "$bviewer"; do
  for _ in $(seq 50); do kill -0 "$pid" 2>/dev/null || break; sleep 0.1; done
  kill -0 "$pid" 2>/dev/null && fail "process $pid still running after leave" || pass "process $pid exited"
done

echo "=== trusted control in a share without --allow-control"
nroom="e2e-notrust-$$"
nsid=$(uuidgen | tr '[:upper:]' '[:lower:]')
ngid=$(uuidgen | tr '[:upper:]' '[:lower:]')
nsharer_pass=$("$BIN" dev-pass --room "$nroom" --role sharer --name "Alex Account" --id "$nsid" --ttl 600)
nguest_pass=$("$BIN" dev-pass --room "$nroom" --role guest --name "Sam Account" --id "$ngid" --ttl 600)
NS=$OUT/notrust-share.jsonl
NV=$OUT/notrust-view.jsonl
mkfifo "$OUT/notrust-share.in" "$OUT/notrust-view.in"
TWINCURSOR_PASS=$nsharer_pass "$BIN" share --ipc --audio off --capture pattern --resolution 720 \
  <"$OUT/notrust-share.in" >"$NS" 2>"$OUT/notrust-share.log" &
nsharer=$!
pids+=("$nsharer")
exec 3>"$OUT/notrust-share.in"
check "$NS" '.event == "connection" and .state == "connected"' "sharer connected, control not allowed"
TWINCURSOR_PASS=$nguest_pass "$BIN" view --ipc --audio off --headless \
  <"$OUT/notrust-view.in" >"$NV" 2>"$OUT/notrust-view.log" &
nviewer=$!
pids+=("$nviewer")
exec 4>"$OUT/notrust-view.in"
check "$NS" ".event == \"participants\" and (.participants | any(.identity == \"guest-$ngid\"))" "sharer sees the guest"
echo "{\"cmd\":\"trust_control\",\"identity\":\"guest-$ngid\"}" >&3
check "$NS" ".event == \"error\" and .message == \"This share doesn't allow remote control.\"" "trust_control is an error"
echo '{"cmd":"request_control"}' >&4
check "$NV" '.event == "control" and .state == "asked"' "the guest asks"
if wait_count "$NV" '.event == "control" and .state == "none"' 2; then pass "the request is turned down"; else fail "the request wasn't turned down"; fi
[[ $(count "$NS" '.event == "control_asked" or (.event == "control" and (.state != "none" or (.trusted | length > 0)))') -eq 0 ]] \
  && pass "nothing asked, granted or trusted" || fail "control state changed without --allow-control"
echo '{"cmd":"leave"}' >&4
echo '{"cmd":"leave"}' >&3
check "$NS" '.event == "ended" and .reason == "left"' "sharer left"
exec 3>&- 4>&-
for pid in "$nsharer" "$nviewer"; do
  for _ in $(seq 50); do kill -0 "$pid" 2>/dev/null || break; sleep 0.1; done
  kill -0 "$pid" 2>/dev/null && fail "process $pid still running after leave" || pass "process $pid exited"
done

echo "=== a pass for the other role fails fast"
wrong() {
  local side=$1 pass_value=$2 message=$3 flags=()
  [[ $side == share ]] && flags=(--capture pattern --resolution 720) || flags=(--headless)
  TWINCURSOR_PASS=$pass_value "$BIN" "$side" --ipc --audio off "${flags[@]}" \
    </dev/null >"$OUT/wrong-$side.jsonl" 2>"$OUT/wrong-$side.log" &
  local pid=$! start end
  start=$(python3 -c 'import time; print(time.time())')
  for _ in $(seq 50); do kill -0 "$pid" 2>/dev/null || break; sleep 0.1; done
  if kill -0 "$pid" 2>/dev/null; then
    kill -INT "$pid" 2>/dev/null || true
    fail "$side with the wrong pass still running after 5 s"
    return
  fi
  local code=0
  wait "$pid" || code=$?
  end=$(python3 -c 'import time; print(time.time())')
  ms=$(python3 -c "print(round(($end - $start) * 1000))")
  if [[ $code -ne 0 ]] && jq -e -s --arg m "$message" \
    'any(.event == "error" and .message == $m) and (last.event == "ended" and last.reason == "failed") and all(.event != "connection" or .state != "connected")' \
    "$OUT/wrong-$side.jsonl" >/dev/null; then
    pass "$side refuses it in ${ms} ms: $message"
  else
    fail "$side with the wrong pass (exit $code): $(tail -2 "$OUT/wrong-$side.jsonl")"
  fi
}
wrong share "$guest_pass" "This pass is for a guest; it can't be used to share."
wrong view "$sharer_pass" "This pass is for the sharer; it can't be used to view."

echo "=== one window shared: control is never given (needs Screen Recording; skipped without it)"
# Captured headless: nothing opens, nothing is saved, and nothing leaves this Mac. WINDOW=<id>
# picks the window (see `tc-engine windows`); by default a Finder or terminal window if there is
# one, else the smallest.
win=${WINDOW:-$("$BIN" windows 2>/dev/null | jq -r '
  (map(select(.app | test("^(Finder|Terminal|iTerm2|Ghostty|WezTerm)$"))) | min_by(.pixels[0] * .pixels[1]))
  // min_by(.pixels[0] * .pixels[1]) | .id // empty' 2>/dev/null || true)}
if [[ -z $win ]]; then
  echo "  skipped: no window to share (is Screen Recording off for this terminal?)"
else
  room="e2e-window-$$"
  WS=$OUT/window-share.jsonl WV=$OUT/window-view.jsonl
  one_window="Control isn't available while one window is shared."
  mkfifo "$OUT/window-share.in" "$OUT/window-view.in"
  "$BIN" share --ipc --audio off --room "$room" --name Alex --window "$win" --resolution 720 \
    --allow-control --inject "record:$OUT/window-actions.jsonl" --headless \
    <"$OUT/window-share.in" >"$WS" 2>"$OUT/window-share.log" &
  wsharer=$!
  pids+=("$wsharer")
  exec 3>"$OUT/window-share.in"
  check "$WS" '.event == "connection" and .state == "connected"' "a share of window $win connects with --allow-control"
  "$BIN" view --ipc --audio off --room "$room" --name Sam --headless \
    <"$OUT/window-view.in" >"$WV" 2>"$OUT/window-view.log" &
  wviewer=$!
  pids+=("$wviewer")
  exec 4>"$OUT/window-view.in"
  check "$WS" '.event == "participants" and (.participants | any(.name == "Sam" and .role == "guest"))' "the sharer sees Sam"
  wguest=$(first "$WS" '.event == "participants" and (.participants | any(.name == "Sam"))' | jq -r '.participants[] | select(.name == "Sam") | .identity')
  echo '{"cmd":"request_control"}' >&4
  check "$WV" ".event == \"error\" and .message == \"$one_window\"" "the guest is told: $one_window"
  check "$WS" ".event == \"error\" and .message == \"$one_window\"" "so is the sharer's app"
  echo "{\"cmd\":\"trust_control\",\"identity\":\"$wguest\"}" >&3
  echo "{\"cmd\":\"trust_on_arrival\",\"identity\":\"guest-later\"}" >&3
  echo '{"cmd":"request_control"}' >&4
  if wait_count "$WS" ".event == \"error\" and .message == \"$one_window\"" 4; then
    pass "trust_control, trust_on_arrival and a second request: the same sentence each"
  else
    fail "expected the sentence four times on the sharer, got $(count "$WS" ".event == \"error\" and .message == \"$one_window\"")"
  fi
  sleep 1
  [[ $(count "$WS" '.event == "control_asked"') -eq 0 ]] && pass "nobody is asked" || fail "the app was asked"
  [[ $(count "$WS" '.event == "control" and .state == "controlling"') -eq 0 && $(count "$WV" '.event == "control" and .state == "controlling"') -eq 0 ]] \
    && pass "nobody ever controls" || fail "someone got control of a window share"
  [[ ! -s $OUT/window-actions.jsonl ]] && pass "nothing was injected" || fail "actions recorded: $(head -3 "$OUT/window-actions.jsonl")"
  echo '{"cmd":"leave"}' >&4
  echo '{"cmd":"leave"}' >&3
  check "$WS" '.event == "ended" and .reason == "left"' "the window share ended"
  exec 3>&- 4>&-
fi

if [[ $failed -eq 0 ]]; then echo "engine-e2e: PASS"; else echo "engine-e2e: FAIL (events and logs in $OUT)"; fi
exit $failed
