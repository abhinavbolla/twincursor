#!/usr/bin/env bash
# Accounts to a call, end to end on one machine, with nobody at the Mac: the local backend
# (Supabase), local LiveKit, `tc-account` and `tc-engine`, and nothing else.
#
#   1. two people sign in (email and password, which only local stacks allow) and see their profiles;
#   2. Alex starts a call; Sam knocks with its code and is listed as knocking; Sam's pass is refused;
#   3. Alex lets Sam in; each is now the other's contact;
#   4. both fetch call passes and join with `tc-engine` (pattern capture, no audio, input recorded,
#      never injected); each sees the other under their account's name and role, and Sam gets video;
#   5. Kim, never let in, knocks and is refused a pass; Sam's pass given to `share` fails fast;
#   6. Alex ends the call and Sam's pass is refused; signing out removes the session file;
#   7. no access token, refresh token or pass appears in anything captured, except the output of
#      `tc-account pass --print-env`, which exists to hand a pass to the engine.
#
# Also checks the sign-in page the tool builds is one the local auth server accepts (it redirects
# to GitHub; the bench follows no further).
#
# Needs a running local stack (`supabase start`), jq, curl, openssl, and a release build:
#   cargo build --release -p twincursor-account -p twincursor-engine-cli
# Starts `livekit-server --dev` if none is running (and stops only that one), and serves the
# call-pass function with the dev server's well-known key while it runs.
# Usage: bench/account-e2e.sh   (BIN=path/to/tc-engine ACCOUNT=path/to/tc-account for other builds)
set -euo pipefail

cd "$(dirname "$0")/.."
BIN=${BIN:-target/release/tc-engine}
ACCOUNT=${ACCOUNT:-target/release/tc-account}
OUT=${OUT:-bench/results/account-e2e-$(date +%s)}
mkdir -p "$OUT"
chmod 700 "$OUT"

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

pids=()
serve_pid=""
livekit_pid=""
cleanup() {
  for pid in "${pids[@]}"; do kill -INT "$pid" 2>/dev/null || true; done
  exec 3>&- 4>&- 2>/dev/null || true
  if [[ -n $serve_pid ]]; then kill "$serve_pid" 2>/dev/null || true; wait "$serve_pid" 2>/dev/null || true; fi
  if [[ -n $livekit_pid ]]; then kill "$livekit_pid" 2>/dev/null || true; wait "$livekit_pid" 2>/dev/null || true; fi
  # The secrets the leak check looked for, and the passes handed to the engine.
  rm -f "$OUT/secrets" "$OUT"/*.env "$OUT/functions.env"
}
trap cleanup EXIT

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

# The dev server's well-known key and secret (livekit-server --dev).
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

# tc-account for one person; every answer is kept in tc-account.log for the leak check, and the
# session's current tokens in `secrets`.
acct() {
  local who=$1
  shift
  local out code=0
  out=$("$ACCOUNT" --session "$OUT/$who.session" "$@" 2>>"$OUT/tc-account.stderr") || code=$?
  # --print-env's answer is the pass itself, which goes to the engine and nowhere else.
  if [[ " $* " == *" --print-env "* ]]; then
    printf '%s %s -> (pass for the engine, not kept)\n' "$who" "$*" >>"$OUT/tc-account.log"
  else
    printf '%s %s -> %s\n' "$who" "$*" "$out" >>"$OUT/tc-account.log"
  fi
  if [[ -s $OUT/$who.session ]]; then
    jq -r '.access_token, .refresh_token' "$OUT/$who.session" >>"$OUT/secrets"
  fi
  printf '%s\n' "$out"
  return $code
}
# Expects tc-account to fail with this kind and sentence.
refused() {
  local label=$1 kind=$2 sentence=$3 out
  shift 3
  if out=$(acct "$@"); then
    fail "$label: it worked"
  elif jq -e --arg k "$kind" --arg s "$sentence" '.kind == $k and .error == $s' <<<"$out" >/dev/null; then
    pass "$label: \"$sentence\""
  else
    fail "$label: $out"
  fi
}

new_user() {
  local email=$1 name=$2 password=$3
  curl -s -X POST "$API_URL/auth/v1/admin/users" \
    -H "apikey: $SERVICE_ROLE_KEY" -H "Authorization: Bearer $SERVICE_ROLE_KEY" -H 'content-type: application/json' \
    -d "$(jq -n --arg e "$email" --arg p "$password" --arg n "$name" '{email: $e, password: $p, email_confirm: true, user_metadata: {name: $n}}')" \
    | jq -r '.id // empty'
}

echo "=== sign-in"
url=$(acct alex authorize-url | jq -r .url)
location=$(curl -s -o /dev/null -w '%{redirect_url}' "$url")
[[ $location == https://github.com/login/oauth/authorize\?* ]] \
  && pass "the sign-in page the tool builds redirects to GitHub" || fail "authorize answered: $location"
refused "whoami before signing in" not_signed_in "You're not signed in." alex whoami

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
TC_ACCOUNT_PASSWORD=wrong-password refused "a wrong password" wrong_password \
  "That email and password don't match an account." alex password-sign-in --email "alex-$run@example.test"
unset TC_ACCOUNT_PASSWORD
mode=$(stat -f '%Lp' "$OUT/alex.session")
[[ $mode == 600 ]] && pass "the session file is readable by its owner only (mode $mode)" || fail "session file mode $mode"
acct alex whoami | jq -e --arg id "$alex_id" '.id == $id and .name == "Alex Account"' >/dev/null \
  && pass "whoami: Alex Account" || fail "Alex's whoami"
acct sam whoami | jq -e --arg id "$sam_id" '.id == $id and .name == "Sam Account"' >/dev/null \
  && pass "whoami: Sam Account" || fail "Sam's whoami"

echo "=== a knock"
started=$(acct alex start-call)
call=$(jq -r .call_id <<<"$started")
code=$(jq -r .code <<<"$started")
[[ $call =~ ^[0-9a-f-]{36}$ && $code =~ ^[0-9a-f]{32}$ ]] && pass "Alex started a call" || fail "start-call: $started"
acct sam knock "$code" | jq -e --arg c "$call" '.call_id == $c and .state == "knocking" and .host.name == "Alex Account"' >/dev/null \
  && pass "Sam knocks on Alex Account's call: knocking" || fail "Sam's knock"
acct alex inbox --hosting "$call" \
  | jq -e --arg s "$sam_id" '.hosting.members | any(.user_id == $s and .name == "Sam Account" and .state == "knocking")' >/dev/null \
  && pass "Alex's inbox lists Sam, knocking" || fail "Alex's inbox"
refused "Sam's pass before being let in" not_allowed \
  "You can't join that call: it has ended, or the host hasn't let you in." sam pass "$call"
refused "a knock on your own call" own_call "That's your own call." alex knock "$code"

echo "=== let in"
acct alex answer-knock "$call" "$sam_id" --let-in | jq -e '.state == "admitted"' >/dev/null \
  && pass "Alex lets Sam in: admitted" || fail "answer-knock"
acct alex inbox | jq -e --arg s "$sam_id" '.contacts | any(.id == $s)' >/dev/null \
  && pass "Sam is Alex's contact" || fail "Sam isn't Alex's contact"
acct sam inbox | jq -e --arg a "$alex_id" '(.contacts | any(.id == $a)) and (.waiting | any(.state == "admitted"))' >/dev/null \
  && pass "Alex is Sam's contact, and Sam sees the knock admitted" || fail "Sam's inbox"
refused "answering the same knock twice" not_knocking "That person isn't waiting to join any more." \
  alex answer-knock "$call" "$sam_id" --turn-away

echo "=== passes into the engine"
acct alex pass "$call" | jq -e --arg a "sharer-$alex_id" '.identity == $a and .role == "sharer" and .name == "Alex Account" and (has("token") | not)' >/dev/null \
  && pass "Alex's pass: sharer-<id>, Alex Account, no token printed" || fail "Alex's pass"
acct sam pass "$call" | jq -e --arg s "guest-$sam_id" '.identity == $s and .role == "guest" and .name == "Sam Account" and (has("token") | not)' >/dev/null \
  && pass "Sam's pass: guest-<id>, Sam Account, no token printed" || fail "Sam's pass"
acct alex pass "$call" --print-env >"$OUT/alex.env"
acct sam pass "$call" --print-env >"$OUT/sam.env"
for who in alex sam; do
  (set -u; . "$OUT/$who.env"; [[ $TWINCURSOR_SERVER_URL == ws://127.0.0.1:7880 && ${TWINCURSOR_PASS:-} == *.*.* ]]) \
    && pass "$who's --print-env sets TWINCURSOR_SERVER_URL and TWINCURSOR_PASS" || fail "$who's --print-env"
done

mkfifo "$OUT/share.in" "$OUT/view.in"
(. "$OUT/alex.env"; export TWINCURSOR_SERVER_URL TWINCURSOR_PASS
  exec "$BIN" share --ipc --audio off --capture pattern --resolution 720 \
    --allow-control --inject "record:$OUT/actions.jsonl") \
  <"$OUT/share.in" >"$OUT/share.jsonl" 2>"$OUT/share.log" &
pids+=($!)
exec 3>"$OUT/share.in"
check "$OUT/share.jsonl" '.event == "connection" and .state == "connected"' "Alex's engine connected with Alex's pass"
(. "$OUT/sam.env"; export TWINCURSOR_SERVER_URL TWINCURSOR_PASS
  exec "$BIN" view --ipc --audio off --headless) \
  <"$OUT/view.in" >"$OUT/view.jsonl" 2>"$OUT/view.log" &
pids+=($!)
exec 4>"$OUT/view.in"
check "$OUT/view.jsonl" '.event == "connection" and .state == "connected"' "Sam's engine connected with Sam's pass"
check "$OUT/share.jsonl" ".event == \"participants\" and (.participants | any(.identity == \"guest-$sam_id\" and .name == \"Sam Account\" and .role == \"guest\"))" \
  "Alex sees Sam Account, a guest"
check "$OUT/view.jsonl" ".event == \"participants\" and (.participants | any(.identity == \"sharer-$alex_id\" and .name == \"Alex Account\" and .role == \"sharer\"))" \
  "Sam sees Alex Account, the sharer"
check "$OUT/view.jsonl" '.event == "stats" and .fps > 0' "Sam receives video"
# The host's heartbeat, as the app sends it while sharing.
acct alex inbox --hosting "$call" | jq -e --arg c "$call" '.hosting.call_id == $c' >/dev/null \
  && pass "Alex's heartbeat keeps the call live" || fail "heartbeat"

echo "=== someone who was never let in"
acct kim knock "$code" | jq -e '.state == "knocking"' >/dev/null && pass "Kim knocks: knocking" || fail "Kim's knock"
refused "Kim's pass" not_allowed "You can't join that call: it has ended, or the host hasn't let you in." kim pass "$call"
(. "$OUT/sam.env"; export TWINCURSOR_SERVER_URL TWINCURSOR_PASS
  exec "$BIN" share --ipc --audio off --capture pattern --resolution 720) \
  </dev/null >"$OUT/wrong-share.jsonl" 2>"$OUT/wrong-share.log" &
wrong=$!
for _ in $(seq 50); do kill -0 "$wrong" 2>/dev/null || break; sleep 0.1; done
if kill -0 "$wrong" 2>/dev/null; then
  kill -INT "$wrong" 2>/dev/null || true
  fail "share with Sam's pass still running after 5 s"
elif jq -e -s 'any(.event == "error" and .message == "This pass is for a guest; it can'"'"'t be used to share.") and all(.event != "connection" or .state != "connected")' \
  "$OUT/wrong-share.jsonl" >/dev/null; then
  pass "share with Sam's (guest) pass fails fast, never connecting"
else
  fail "share with Sam's pass: $(tail -2 "$OUT/wrong-share.jsonl")"
fi

echo '{"cmd":"leave"}' >&4
check "$OUT/view.jsonl" '.event == "ended" and .reason == "left"' "Sam left"
echo '{"cmd":"leave"}' >&3
check "$OUT/share.jsonl" '.event == "ended" and .reason == "left"' "Alex left"
exec 3>&- 4>&-

echo "=== the end of the call, and signing out"
acct alex end "$call" | jq -e '.ok' >/dev/null && pass "Alex ends the call" || fail "end"
refused "Sam's pass after the end" not_allowed "You can't join that call: it has ended, or the host hasn't let you in." sam pass "$call"
refused "a knock on the ended call" call_not_found "That call has ended or the link is wrong." kim knock "$code"
for who in sam alex kim; do
  acct "$who" sign-out | jq -e '.signed_in == false' >/dev/null || fail "$who's sign-out"
done
[[ ! -e $OUT/sam.session && ! -e $OUT/alex.session && ! -e $OUT/kim.session ]] \
  && pass "sign-out removes the session files" || fail "a session file is still there"
refused "whoami after signing out" not_signed_in "You're not signed in." sam whoami

echo "=== no secret in anything captured"
secrets=$(sort -u "$OUT/secrets")
for who in alex sam; do
  secrets+=$'\n'$(sed -n "s/^TWINCURSOR_PASS='\(.*\)'$/\1/p" "$OUT/$who.env")
done
parts=0
leaked=""
captured=("$OUT/tc-account.log" "$OUT/tc-account.stderr" "$OUT"/*.jsonl "$OUT"/*.log)
while IFS= read -r secret; do
  [[ -z $secret ]] && continue
  # A JWT leaks if any of its parts does; a refresh token is one part.
  for part in $(tr '.' ' ' <<<"$secret"); do
    [[ ${#part} -lt 8 ]] && continue
    parts=$((parts + 1))
    if grep -qF -- "$part" "${captured[@]}" 2>/dev/null; then leaked+=" $(grep -lF -- "$part" "${captured[@]}" | xargs -n1 basename | sort -u | tr '\n' ' ')"; fi
  done
done <<<"$secrets"
[[ $parts -gt 10 ]] || fail "too few secrets collected to check ($parts)"
[[ -z $leaked ]] && pass "none of $parts token parts appears in tc-account's answers, the engines' events or any log" \
  || fail "a token leaked into:$leaked"

if [[ $failed -eq 0 ]]; then echo "account-e2e: PASS"; else echo "account-e2e: FAIL (events and logs in $OUT)"; fi
exit $failed
