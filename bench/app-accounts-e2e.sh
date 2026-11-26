#!/usr/bin/env bash
# The desktop app's calls with accounts, end to end on one Mac, with nobody at it.
#
# The real app (`twincursor-desktop`) runs in its test-only script mode (`--script PATH --report
# PATH`, accounts flavour): signed in with a session file (TWINCURSOR_SESSION_FILE), sharing the
# synthetic pattern with voice off, recording input instead of injecting it, with a clipboard in
# memory. `tc-account` and `tc-engine` play the other person. Against the local backend
# (`supabase start`) and local LiveKit:
#
#   a. host app: Share starts a call with a pass (no --room, nothing secret in the flags); Sam
#      knocks with the link's code; letting him in at once is refused; after a second he is let
#      in with control; he joins, the app trusts him once when he appears, his request is
#      granted with no prompt, his typing is recorded; after a revoke his next request is a
#      prompt; ending the call stops his passes;
#   b. let in without control: his request is a prompt, an allow at once is refused, later taken;
#   c. turned away: no pass, no trust, nothing injected;
#   d. guest app: Sam hosts; the app joins by link, knocks, is let in, gets video; Sam ends the
#      call and the app leaves by itself with "The call ended.";
#   e. rings: the app rings Sam, who sees it and gets a pass; Sam rings the app, which joins;
#   f. HEVC fallback: a guest without HEVC joins a 4K share; the app restarts its engine on
#      H.264 with a fresh pass, and the guest it had trusted must ask again;
#   g. no access token, refresh token or pass in any report, log or engine flag.
#
# Needs the local stack running (`supabase start`), jq, curl, openssl, and release builds:
#   (cd apps/desktop && npm ci && npm run build)
#   cargo build --release -p twincursor-desktop -p twincursor-engine-cli -p twincursor-media-spike -p twincursor-account
# Starts `livekit-server --dev` if none is running (and stops only that one), and serves the
# call-pass function with the dev server's well-known key while it runs.
# Usage: bench/app-accounts-e2e.sh   (TARGET=dir, or APP= BIN= SPIKE= ACCOUNT= for other builds)
set -euo pipefail

cd "$(dirname "$0")/.."
TARGET=${TARGET:-${CARGO_TARGET_DIR:-target}}
APP=${APP:-$TARGET/release/twincursor-desktop}
BIN=${BIN:-$TARGET/release/tc-engine}
SPIKE=${SPIKE:-$TARGET/release/tc-spike}
ACCOUNT=${ACCOUNT:-$TARGET/release/tc-account}
OUT=${OUT:-bench/results/app-accounts-e2e-$(date +%s)}
mkdir -p "$OUT"
chmod 700 "$OUT"
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
first() { jq -c -s "map(select($2)) | first" "$1"; }
ev() { echo ".kind == \"event\" and ($1)"; }
home() { echo ".kind == \"home\" and ($1)"; }
trace() { echo ".kind == \"trace\" and ($1)"; }
running() { kill -0 "$1" 2>/dev/null; }

pids=()
serve_pid=""
livekit_pid=""
cleanup() {
  for pid in "${pids[@]}"; do kill -INT "$pid" 2>/dev/null || true; done
  exec 3>&- 4>&- 2>/dev/null || true
  if [[ -n $serve_pid ]]; then kill "$serve_pid" 2>/dev/null || true; wait "$serve_pid" 2>/dev/null || true; fi
  if [[ -n $livekit_pid ]]; then kill "$livekit_pid" 2>/dev/null || true; wait "$livekit_pid" 2>/dev/null || true; fi
  rm -f "$OUT/secrets" "$OUT"/*.env "$OUT/functions.env"
}
trap cleanup EXIT

for tool in "$APP" "$BIN" "$SPIKE" "$ACCOUNT"; do
  [[ -x $tool ]] || { echo "Missing $tool: build it first (see the top of this script)." >&2; exit 1; }
done
eval "$(supabase status -o env 2>/dev/null | grep -E '^(API_URL|ANON_KEY|SERVICE_ROLE_KEY)=')"
if [[ -z ${API_URL:-} ]]; then
  echo "The local stack isn't running: run 'supabase start' first." >&2
  exit 1
fi
export TWINCURSOR_SUPABASE_URL=$API_URL TWINCURSOR_SUPABASE_KEY=$ANON_KEY

echo "=== the local backend and LiveKit (results in $OUT)"
if pgrep -x livekit-server >/dev/null; then
  pass "livekit-server is already running"
else
  livekit-server --dev >"$OUT/livekit.log" 2>&1 &
  livekit_pid=$!
  for _ in $(seq 50); do nc -z 127.0.0.1 7880 2>/dev/null && break; sleep 0.1; done
  nc -z 127.0.0.1 7880 && pass "started livekit-server --dev" || { fail "livekit-server didn't start"; exit 1; }
fi
cat >"$OUT/functions.env" <<'EOF'
LIVEKIT_URL=ws://127.0.0.1:7880
LIVEKIT_API_KEY=devkey
LIVEKIT_API_SECRET=secret
EOF
supabase functions serve --env-file "$OUT/functions.env" >"$OUT/serve.log" 2>&1 &
serve_pid=$!
status=""
for _ in $(seq 60); do
  status=$(curl -s -o /dev/null -w '%{http_code}' -X POST "$API_URL/functions/v1/call-pass" \
    -H "apikey: $ANON_KEY" -H 'content-type: application/json' -d '{}' || true)
  [[ $status == 401 ]] && break
  sleep 1
done
[[ $status == 401 ]] && pass "call-pass is served" || { fail "call-pass didn't come up (HTTP ${status:-none})"; exit 1; }

# tc-account for one person; answers kept for the leak check, the session's tokens in `secrets`.
acct() {
  local who=$1
  shift
  local out code=0
  out=$("$ACCOUNT" --session "$OUT/$who.session" "$@" 2>>"$OUT/tc-account.stderr") || code=$?
  if [[ " $* " == *" --print-env "* ]]; then
    printf '%s %s -> (pass for the engine, not kept)\n' "$who" "$*" >>"$OUT/tc-account.log"
  else
    printf '%s %s -> %s\n' "$who" "$*" "$out" >>"$OUT/tc-account.log"
  fi
  keep_tokens "$who"
  printf '%s\n' "$out"
  return $code
}
keep_tokens() {
  [[ -s $OUT/$1.session ]] && jq -r '.access_token, .refresh_token' "$OUT/$1.session" >>"$OUT/secrets" || true
}
not_allowed() { # label, who, call
  local out
  if out=$(acct "$2" pass "$3"); then fail "$1: a pass was given"
  elif jq -e '.kind == "not_allowed"' <<<"$out" >/dev/null; then pass "$1"
  else fail "$1: $out"; fi
}
new_user() {
  curl -s -X POST "$API_URL/auth/v1/admin/users" \
    -H "apikey: $SERVICE_ROLE_KEY" -H "Authorization: Bearer $SERVICE_ROLE_KEY" -H 'content-type: application/json' \
    -d "$(jq -n --arg e "$1" --arg p "$3" --arg n "$2" '{email: $e, password: $p, email_confirm: true, user_metadata: {name: $n}}')" \
    | jq -r '.id // empty'
}

# The app, in script mode, as Alex. $1 names the case; the script is in $OUT/$1.json.
app_run() {
  TWINCURSOR_SESSION_FILE="$OUT/alex.session" "$APP" --script "$OUT/$1.json" --report "$OUT/$1.jsonl" \
    2>"$OUT/$1-app.log" &
  app=$!
  pids+=("$app")
}
app_done() { # case
  for _ in $(seq 150); do running "$app" || break; sleep 0.1; done
  running "$app" && { fail "$1: the app didn't quit after its script"; kill -INT "$app"; }
  check "$OUT/$1.jsonl" '.kind == "script" and .done and .ok' "$1: every script step passed" 1
  keep_tokens alex
}
# The engine flags a case's app started with: a pass, and nothing secret or room-coded.
flags_ok() { # case, role, extra flags...
  local c=$1 role=$2 started args
  shift 2
  started=$(first "$OUT/$c.jsonl" ".kind == \"started\" and .role == \"$role\"")
  args=$(jq -r '.args | join(" ")' <<<"$started")
  for flag in "$@"; do
    [[ " $args " == *" $flag "* ]] && pass "$c: engine flag $flag" || fail "$c: engine flags lack $flag: $args"
  done
  [[ $args != *--room* && $args != *--name* && $args != *secret* && $args != *eyJ* ]] \
    && pass "$c: no room, name, secret or pass among the engine's flags" || fail "$c: flags: $args"
}
hosting_code() { jq -r -s 'map(select(.kind == "home" and .home.stage.kind == "hosting")) | first | .home.stage.link | sub("^twincursor://call/"; "")' "$OUT/$1.jsonl"; }
hosting_call() { jq -r -s 'map(select(.kind == "home" and .home.stage.kind == "hosting")) | first | .home.stage.call_id' "$OUT/$1.jsonl"; }

# A headless guest with a pass: $1 a name for its files, $2 who, $3 the call, more flags after.
guest_start() {
  local name=$1 who=$2 call=$3
  shift 3
  acct "$who" pass "$call" --print-env >"$OUT/$name.env"
  rm -f "$OUT/$name.in"
  mkfifo "$OUT/$name.in"
  (. "$OUT/$name.env"; export TWINCURSOR_SERVER_URL TWINCURSOR_PASS
    exec "$BIN" view --ipc --audio off --headless "$@") \
    <"$OUT/$name.in" >"$OUT/$name.jsonl" 2>"$OUT/$name.log" &
  guest=$!
  pids+=("$guest")
  exec 4>"$OUT/$name.in"
}
guest_stop() { # name
  echo '{"cmd":"leave"}' >&4 2>/dev/null || true
  wait_for "$OUT/$1.jsonl" '.event == "ended"' 10 || true
  exec 4>&-
}

echo "=== people"
run=$$-$RANDOM
password=$(openssl rand -hex 16)
alex_id=$(new_user "alex-$run@example.test" "Alex Account" "$password")
sam_id=$(new_user "sam-$run@example.test" "Sam Account" "$password")
kim_id=$(new_user "kim-$run@example.test" "Kim Account" "$password")
[[ -n $alex_id && -n $sam_id && -n $kim_id ]] && pass "three users made through the admin API" || { fail "couldn't make users"; exit 1; }
export TC_ACCOUNT_PASSWORD=$password
for who in alex sam kim; do
  acct "$who" password-sign-in --email "$who-$run@example.test" >/dev/null || fail "$who couldn't sign in"
done
unset TC_ACCOUNT_PASSWORD
# Types once in control; then stays, so later requests from stdin still go through.
cat >"$OUT/guest-types.json" <<'EOF'
[{"do":"await_control"},{"do":"type","text":"hello","gap_ms":20},{"do":"await_end"},{"do":"wait","ms":600000}]
EOF

echo "=== a. the app hosts: knock, arming, let in with control, revoke, end"
cat >"$OUT/a.json" <<EOF
{
  "accounts": {"quality": "1080p", "inject_record": "$OUT/a-actions.jsonl"},
  "steps": [
    {"do": "share"},
    {"do": "wait_knock", "name": "Sam Account"},
    {"do": "answer_knock", "name": "Sam Account", "let_in": true, "control": true, "try_early": true},
    {"do": "wait_for", "event": {"event": "control", "state": "controlling"}},
    {"do": "wait_ms", "ms": 2500},
    {"do": "send", "cmd": "revoke_control"},
    {"do": "answer", "with": "deny"},
    {"do": "wait_ms", "ms": 500},
    {"do": "end"},
    {"do": "wait_home", "home": {"stage": {"kind": "idle"}}}
  ],
  "quit": true
}
EOF
app_run a
check "$OUT/a.jsonl" "$(home '.home.stage.kind == "hosting" and (.home.stage.link | test("^twincursor://call/[0-9a-f]{32}$"))')" "a: Share: the app is hosting, with a call link"
check "$OUT/a.jsonl" "$(home '.home.notice == "The call link is on your clipboard."')" "a: it says the link is on the clipboard"
flags_ok a share "--allow-control" "--exclude-app-pid=$app" "--audio=off" "--capture=pattern" "--inject=record:$OUT/a-actions.jsonl"
check "$OUT/a.jsonl" "$(trace '.trace.kind == "pass" and .trace.reason == "start" and .trace.identity == "sharer-'"$alex_id"'"')" "a: the engine started with Alex's sharer pass"
code=$(hosting_code a)
call=$(hosting_call a)
acct sam knock "$code" | jq -e '.state == "knocking" and .host.name == "Alex Account"' >/dev/null \
  && pass "a: Sam knocks with the link's code" || fail "a: Sam's knock"
check "$OUT/a.jsonl" "$(home '.home.knocks | any(.name == "Sam Account" and .user_id == "'"$sam_id"'")')" "a: the app shows Sam knocking"
check "$OUT/a.jsonl" '.kind == "script" and .early_refused == "Too soon. Try again."' "a: letting Sam in at once is refused (too soon)"
check "$OUT/a.jsonl" "$(trace '.trace.kind == "let_in" and .trace.control == true')" "a: after a second, Sam is let in with control"
guest_start a-guest sam "$call" --control-script "$OUT/guest-types.json" --sent-log "$OUT/a-sent.jsonl"
check "$OUT/a-guest.jsonl" '.event == "connection" and .state == "connected"' "a: Sam joins with his own pass"
check "$OUT/a.jsonl" "$(trace '.trace.kind == "trust_sent" and .trace.why == "arrived" and .trace.identity == "guest-'"$sam_id"'"')" "a: the app gives Sam control when he appears"
check "$OUT/a.jsonl" "$(ev '.event.event == "control" and (.event.trusted | any(.identity == "guest-'"$sam_id"'"))')" "a: the engine trusts Sam"
echo '{"cmd":"request_control"}' >&4
check "$OUT/a-guest.jsonl" '.event == "control" and .state == "controlling"' "a: Sam's request is granted"
check "$OUT/a.jsonl" "$(ev '.event.event == "control" and .event.state == "controlling" and .event.by.name == "Sam Account"')" "a: the app shows Sam Account controlling"
[[ $(count "$OUT/a.jsonl" "$(ev '.event.event == "control_asked"')") -eq 0 ]] && pass "a: with no control_asked" || fail "a: a control_asked for a trusted guest"
[[ $(count "$OUT/a.jsonl" "$(trace '.trace.kind == "trust_sent"')") -eq 1 ]] && pass "a: trust_control was sent once" || fail "a: trust_control sent $(count "$OUT/a.jsonl" "$(trace '.trace.kind == "trust_sent"')") times"
for _ in $(seq 100); do [[ -s $OUT/a-actions.jsonl ]] && [[ $(grep -c '"text"' "$OUT/a-actions.jsonl" || true) -ge 5 ]] && break; sleep 0.1; done
check "$OUT/a.jsonl" '.kind == "script" and .step == 5 and .ok' "a: the app takes control back (revoke_control)"
untrusted=0
for _ in $(seq 50); do
  jq -e -s 'map(select(.kind == "event" and .event.event == "control")) | last | .event.state == "none" and .event.trusted == []' "$OUT/a.jsonl" >/dev/null 2>&1 && { untrusted=1; break; }
  sleep 0.1
done
[[ $untrusted -eq 1 ]] && pass "a: nobody in control and nobody trusted after it" || fail "a: control or trust after the revoke"
if "$SPIKE" replay --sent "$OUT/a-sent.jsonl" --actions "$OUT/a-actions.jsonl" >"$OUT/a-replay.log" 2>&1; then
  pass "a: what was recorded is what Sam typed ($(tail -1 "$OUT/a-replay.log"))"
else
  fail "a: replay: $(tail -3 "$OUT/a-replay.log")"
fi
wait_for "$OUT/a-guest.jsonl" '.event == "control" and .state == "none"' 10 || true
echo '{"cmd":"request_control"}' >&4
check "$OUT/a.jsonl" "$(ev '.event.event == "control_asked" and .event.identity == "guest-'"$sam_id"'"')" "a: after the revoke, Sam's next request is a prompt"
check "$OUT/a.jsonl" '.kind == "script" and .step == 6 and .ok' "a: and the app denies it"
check "$OUT/a.jsonl" '.kind == "exited"' "a: End: the app's engine exits" 40
app_done a
acct alex inbox | jq -e '.hosting == null' >/dev/null && pass "a: the call is over on the backend" || fail "a: the call is still open"
not_allowed "a: Sam can't get a pass after the end" sam "$call"
guest_stop a-guest

echo "=== b. let in without control: the request is a prompt, and an allow at once is refused"
cat >"$OUT/b.json" <<EOF
{
  "accounts": {"quality": "1080p", "inject_record": "$OUT/b-actions.jsonl"},
  "steps": [
    {"do": "share"},
    {"do": "wait_knock", "name": "Sam Account"},
    {"do": "answer_knock", "name": "Sam Account", "let_in": true, "control": false},
    {"do": "answer", "with": "allow", "early": true},
    {"do": "wait_for", "event": {"event": "control", "state": "controlling"}},
    {"do": "wait_ms", "ms": 1000},
    {"do": "end"}
  ],
  "quit": true
}
EOF
app_run b
check "$OUT/b.jsonl" "$(home '.home.stage.kind == "hosting"')" "b: hosting"
code=$(hosting_code b)
call=$(hosting_call b)
acct sam knock "$code" >/dev/null || fail "b: Sam's knock"
check "$OUT/b.jsonl" "$(trace '.trace.kind == "let_in" and .trace.control == false')" "b: Sam is let in without control"
guest_start b-guest sam "$call" --control-script "$OUT/guest-types.json" --sent-log "$OUT/b-sent.jsonl"
check "$OUT/b.jsonl" "$(ev '.event.event == "participants" and (.event.participants | any(.identity == "guest-'"$sam_id"'"))')" "b: Sam is in the call"
sleep 1
echo '{"cmd":"request_control"}' >&4
check "$OUT/b.jsonl" "$(ev '.event.event == "control_asked" and .event.name == "Sam Account"')" "b: Sam's request raises control_asked"
check "$OUT/b.jsonl" '.kind == "script" and .early_refused == "Too soon. Try again."' "b: an allow sent at once is refused"
check "$OUT/b-guest.jsonl" '.event == "control" and .state == "controlling"' "b: the allow a second later is taken"
[[ $(count "$OUT/b.jsonl" "$(trace '.trace.kind == "trust_sent"')") -eq 0 ]] && pass "b: no trust was given" || fail "b: trust was given"
app_done b
guest_stop b-guest

echo "=== c. turned away"
cat >"$OUT/c.json" <<EOF
{
  "accounts": {"quality": "1080p", "inject_record": "$OUT/c-actions.jsonl"},
  "steps": [
    {"do": "share"},
    {"do": "wait_knock", "name": "Kim Account"},
    {"do": "answer_knock", "name": "Kim Account", "let_in": false},
    {"do": "wait_ms", "ms": 3000},
    {"do": "end"}
  ],
  "quit": true
}
EOF
app_run c
check "$OUT/c.jsonl" "$(home '.home.stage.kind == "hosting"')" "c: hosting"
code=$(hosting_code c)
call=$(hosting_call c)
acct kim knock "$code" >/dev/null || fail "c: Kim's knock"
check "$OUT/c.jsonl" "$(home '.home.members | any(.name == "Kim Account" and .state == "turned_away")')" "c: Kim is turned away at once"
acct kim knock "$code" | jq -e '.state == "turned_away"' >/dev/null && pass "c: Kim's next knock says turned away" || fail "c: Kim's knock after"
not_allowed "c: Kim gets no pass" kim "$call"
app_done c
[[ $(count "$OUT/c.jsonl" "$(trace '.trace.kind == "trust_sent" or .trace.kind == "let_in"')") -eq 0 ]] && pass "c: no trust, nobody let in" || fail "c: trust or let in"
[[ ! -s $OUT/c-actions.jsonl ]] && pass "c: nothing injected" || fail "c: input was recorded"

echo "=== d. the app joins Sam's call by link, and leaves when Sam ends it"
started=$(acct sam start-call)
call=$(jq -r .call_id <<<"$started")
code=$(jq -r .code <<<"$started")
acct sam pass "$call" --print-env >"$OUT/d-host.env"
rm -f "$OUT/d-host.in"
mkfifo "$OUT/d-host.in"
(. "$OUT/d-host.env"; export TWINCURSOR_SERVER_URL TWINCURSOR_PASS
  exec "$BIN" share --ipc --audio off --capture pattern --resolution 720 --allow-control --inject "record:$OUT/d-host-actions.jsonl") \
  <"$OUT/d-host.in" >"$OUT/d-host.jsonl" 2>"$OUT/d-host.log" &
pids+=($!)
exec 3>"$OUT/d-host.in"
check "$OUT/d-host.jsonl" '.event == "connection" and .state == "connected"' "d: Sam shares with his pass"
cat >"$OUT/d.json" <<EOF
{
  "accounts": {"quality": "1080p", "inject_record": "$OUT/d-actions.jsonl"},
  "steps": [
    {"do": "join_link", "link": "twincursor://call/$code"},
    {"do": "wait_home", "home": {"stage": {"kind": "joined", "host": {"name": "Sam Account"}}}},
    {"do": "wait_home", "home": {"stage": {"kind": "idle"}, "notice": "The call ended."}}
  ],
  "quit": true
}
EOF
app_run d
check "$OUT/d.jsonl" "$(home '.home.stage.kind == "knocking" and .home.stage.host.name == "Sam Account"')" "d: the app knocks, and shows whose call it is"
check_knock() { acct sam inbox --hosting "$call" | jq -e --arg a "$alex_id" '.hosting.members | any(.user_id == $a and .state == "knocking")' >/dev/null; }
for _ in $(seq 20); do check_knock && break; sleep 0.5; done
check_knock && pass "d: Sam sees Alex knocking" || fail "d: Sam doesn't see the knock"
sleep 3
check "$OUT/d.jsonl" "$(home '.home.stage.kind == "knocking"')" "d: still knocking until let in (keep-alive)"
acct sam answer-knock "$call" "$alex_id" --let-in | jq -e '.state == "admitted"' >/dev/null && pass "d: Sam lets Alex in" || fail "d: answer-knock"
check "$OUT/d.jsonl" "$(home '.home.stage.kind == "joined"')" "d: the app joins"
flags_ok d join "view" "--audio=off"
check "$OUT/d.jsonl" "$(ev '.event.event == "stats" and .event.fps > 0')" "d: the app receives video" 40
check "$OUT/d-host.jsonl" ".event == \"participants\" and (.participants | any(.identity == \"guest-$alex_id\" and .name == \"Alex Account\"))" "d: Sam sees Alex Account in his call"
acct sam end "$call" | jq -e '.ok' >/dev/null && pass "d: Sam ends the call" || fail "d: end"
check "$OUT/d.jsonl" "$(home '.home.stage.kind == "idle" and .home.notice == "The call ended."')" "d: the app leaves by itself: \"The call ended.\"" 20
check "$OUT/d.jsonl" '.kind == "exited"' "d: the app's engine exits"
app_done d
echo '{"cmd":"leave"}' >&3
exec 3>&-

echo "=== e. rings, both ways"
acct sam inbox | jq -e --arg a "$alex_id" '.contacts | any(.id == $a)' >/dev/null && pass "e: Alex and Sam are contacts" || fail "e: not contacts"
cat >"$OUT/e.json" <<EOF
{
  "accounts": {"quality": "1080p", "inject_record": "$OUT/e-actions.jsonl"},
  "steps": [
    {"do": "ring", "name": "Sam Account"},
    {"do": "wait_home", "home": {"members": [{"name": "Sam Account", "state": "admitted"}]}},
    {"do": "wait_ring", "name": "Sam Account"},
    {"do": "answer_ring", "accept": true},
    {"do": "wait_home", "home": {"stage": {"kind": "joined", "host": {"name": "Sam Account"}}}},
    {"do": "wait_ms", "ms": 4000},
    {"do": "end"}
  ],
  "quit": true
}
EOF
app_run e
check "$OUT/e.jsonl" "$(home '.home.stage.kind == "hosting" and (.home.members | any(.name == "Sam Account" and .state == "ringing"))')" "e: the app shares and rings Sam"
call=$(hosting_call e)
ringing() { acct sam inbox | jq -e --arg c "$call" --arg a "$alex_id" '.rings | any(.call_id == $c and .host.id == $a)' >/dev/null; }
for _ in $(seq 20); do ringing && break; sleep 0.5; done
ringing && pass "e: Sam's inbox shows Alex ringing" || fail "e: no ring in Sam's inbox"
acct sam answer-ring "$call" --accept | jq -e '.state == "admitted"' >/dev/null && pass "e: Sam accepts" || fail "e: answer-ring"
acct sam pass "$call" | jq -e --arg s "guest-$sam_id" '.identity == $s and .role == "guest"' >/dev/null && pass "e: Sam gets a guest pass" || fail "e: Sam's pass"
check "$OUT/e.jsonl" "$(home '.home.members | any(.name == "Sam Account" and .state == "admitted")')" "e: the app shows Sam admitted"
rung=$(acct sam ring "$alex_id")
call2=$(jq -r .call_id <<<"$rung")
[[ $call2 =~ ^[0-9a-f-]{36}$ ]] && pass "e: Sam rings Alex" || fail "e: ring: $rung"
acct sam pass "$call2" --print-env >"$OUT/e-host.env"
rm -f "$OUT/e-host.in"
mkfifo "$OUT/e-host.in"
(. "$OUT/e-host.env"; export TWINCURSOR_SERVER_URL TWINCURSOR_PASS
  exec "$BIN" share --ipc --audio off --capture pattern --resolution 720) \
  <"$OUT/e-host.in" >"$OUT/e-host.jsonl" 2>"$OUT/e-host.log" &
pids+=($!)
exec 3>"$OUT/e-host.in"
check "$OUT/e.jsonl" "$(home '.home.rings | any(.call_id == "'"$call2"'" and .host.name == "Sam Account")')" "e: the app shows Sam ringing"
check "$OUT/e.jsonl" "$(home '.home.stage.kind == "joined" and .home.stage.host.name == "Sam Account"')" "e: accepting joins Sam's call"
check "$OUT/e.jsonl" '.kind == "started" and .role == "join"' "e: as a guest"
check "$OUT/e-host.jsonl" ".event == \"participants\" and (.participants | any(.identity == \"guest-$alex_id\"))" "e: Sam sees Alex in his call" 40
not_allowed "e: accepting ended the app's own call first" sam "$call"
app_done e
echo '{"cmd":"leave"}' >&3
exec 3>&-
acct sam end "$call2" >/dev/null || true

echo "=== f. a guest without HEVC: a restart on H.264 with a fresh pass, and control reset"
cat >"$OUT/f.json" <<EOF
{
  "accounts": {"inject_record": "$OUT/f-actions.jsonl"},
  "steps": [
    {"do": "share"},
    {"do": "wait_knock", "name": "Sam Account"},
    {"do": "answer_knock", "name": "Sam Account", "let_in": true, "control": true},
    {"do": "answer", "with": "deny"},
    {"do": "wait_ms", "ms": 500},
    {"do": "end"}
  ],
  "quit": true
}
EOF
app_run f
check "$OUT/f.jsonl" '.kind == "started" and .codec == "h265"' "f: the app shares at 4K with HEVC"
flags_ok f share "--resolution=2160" "--codec=h265" "--allow-control"
check "$OUT/f.jsonl" "$(home '.home.stage.kind == "hosting"')" "f: hosting"
code=$(hosting_code f)
call=$(hosting_call f)
acct sam knock "$code" >/dev/null || fail "f: Sam's knock"
check "$OUT/f.jsonl" "$(trace '.trace.kind == "let_in" and .trace.control == true')" "f: Sam is let in with control"
guest_start f-guest sam "$call" --decoders h264,vp8
check "$OUT/f.jsonl" '.kind == "restarting" and .codec == "h264"' "f: the app switches to H.264 for Sam"
check "$OUT/f.jsonl" "$(home '.home.notice != null and (.home.notice | test("Control was reset: anyone you gave it to has to be given it again"))')" "f: and says control has to be given again"
check "$OUT/f.jsonl" "$(trace '.trace.kind == "pass" and .trace.reason == "restart"')" "f: the new engine got a fresh pass"
check "$OUT/f.jsonl" '.kind == "started" and .codec == "h264" and any(.args[]; . == "--resolution=2160") and all(.args[]; startswith("--room") | not)' "f: a new engine, same resolution, H.264, still no room flag"
check "$OUT/f-guest.jsonl" '.event == "stats" and .codec == "h264" and .fps > 0' "f: Sam gets video again, in H.264" 40
sleep 2
echo '{"cmd":"request_control"}' >&4
check "$OUT/f.jsonl" "$(ev '.event.event == "control_asked" and .event.identity == "guest-'"$sam_id"'"')" "f: Sam, trusted before, has to ask again"
[[ $(count "$OUT/f.jsonl" '.kind == "restarting"') -eq 1 ]] && pass "f: it switched once" || fail "f: it switched more than once"
[[ $(count "$OUT/f.jsonl" "$(trace '.trace.kind == "trust_sent"')") -le 1 ]] && pass "f: trust_control never sent again after the restart" || fail "f: trust re-sent"
app_done f
guest_stop f-guest

echo "=== g. no token or pass anywhere"
keep_tokens alex
secrets=$(sort -u "$OUT/secrets")
for env in "$OUT"/*.env; do
  [[ $env == *functions.env ]] && continue
  secrets+=$'\n'$(sed -n "s/^TWINCURSOR_PASS='\(.*\)'$/\1/p" "$env")
done
parts=0
leaked=""
captured=("$OUT"/*.jsonl "$OUT"/*.log "$OUT/tc-account.stderr")
while IFS= read -r secret; do
  [[ -z $secret ]] && continue
  for part in $(tr '.' ' ' <<<"$secret"); do
    [[ ${#part} -lt 8 ]] && continue
    parts=$((parts + 1))
    if grep -qF -- "$part" "${captured[@]}" 2>/dev/null; then
      leaked+=" $(grep -lF -- "$part" "${captured[@]}" | xargs -n1 basename | sort -u | tr '\n' ' ')"
    fi
  done
done <<<"$secrets"
[[ $parts -gt 20 ]] || fail "too few secrets collected to check ($parts)"
[[ -z $leaked ]] && pass "none of $parts token and pass parts appears in the app's reports, its logs, the engines' output or flags" \
  || fail "a token leaked into:$leaked"

for who in alex sam kim; do acct "$who" sign-out >/dev/null || true; done
if [[ $failed -eq 0 ]]; then echo "app-accounts-e2e: PASS"; else echo "app-accounts-e2e: FAIL (reports and logs in $OUT)"; fi
exit $failed
