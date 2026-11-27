#!/usr/bin/env bash
# Starts the real Windows app (twincursor-desktop.exe) in its test-only script mode and checks
# what it reports, with nobody at the computer and nothing on screen to click. It runs in CI on
# the windows-latest runner, from Git Bash, after a release build:
#
#   bench/windows-smoke.sh                          # no backend: start, engine found, quit
#   LIVEKIT=path/to/livekit-server.exe bench/windows-smoke.sh   # and a guest joins a room
#
# 1. With no backend named, the app starts, finds tc-engine.exe next to it, says it isn't
#    connected to a server, says it can't share (a Windows app is a guest), and exits 0. Its
#    log file must say the same, and that the windows and the tray icon were made.
#    Both windows must read back from Windows as tool windows: no taskbar button, not in
#    Alt-Tab, topmost.
# 2. With the windows live, the panel and the call bar are brought on screen unprompted (as a
#    ring and a call do). Read back afterwards, neither may have taken the keyboard, and both
#    must still be tool windows; the tray icon must have been made.
# 3. With LIVEKIT set, a LiveKit dev server is started on 127.0.0.1, and the app joins an empty
#    room as a guest (the room-code path, dev keys): it starts its engine, the engine connects,
#    the script leaves, the engine exits, and the app exits 0. No sharer is needed for that. The
#    engine must read back as inside the app's job object.
#
# APP (default target/release/twincursor-desktop.exe) is the app; OUT a folder for the reports
# and logs (a fresh temporary one by default). Nothing here captures, injects or plays sound:
# the app's windows stay hidden, voice is off, and the engine gets no sharer to watch.
set -euo pipefail

APP="${APP:-target/release/twincursor-desktop.exe}"
OUT="${OUT:-$(mktemp -d)}"
LIVEKIT="${LIVEKIT:-}"
mkdir -p "$OUT"

pass() { echo "  ok: $1"; }
fail() {
  echo "  FAIL: $1" >&2
  failed=1
}
failed=0

win() { cygpath -w "$1"; }

[ -f "$APP" ] || { echo "no app at $APP" >&2; exit 2; }
[ -f "$(dirname "$APP")/tc-engine.exe" ] || { echo "no tc-engine.exe next to $APP" >&2; exit 2; }

# Nothing from the environment may name a backend or a server for the first run.
unset TWINCURSOR_SUPABASE_URL TWINCURSOR_SUPABASE_KEY TWINCURSOR_SERVER_URL TWINCURSOR_API_KEY \
  TWINCURSOR_API_SECRET LIVEKIT_URL LIVEKIT_API_KEY LIVEKIT_API_SECRET

# check FILE JQ-FILTER WHAT: some line of the report matches.
check() {
  if jq -e -s "any(.[]; $2)" "$1" >/dev/null; then pass "$3"; else fail "$3"; fi
}
# logged FILE TEXT WHAT: the app's log has a line with TEXT.
logged() {
  if grep -qF -- "$2" "$1"; then pass "$3"; else fail "$3"; fi
}

run_app() { # NAME SCRIPT-JSON
  local name="$1"
  printf '%s\n' "$2" >"$OUT/$name.json"
  local code=0
  TWINCURSOR_LOG_FILE="$(win "$OUT/$name.log")" \
    timeout 180 "$APP" --script "$(win "$OUT/$name.json")" --report "$(win "$OUT/$name.jsonl")" || code=$?
  echo "  app exited with $code"
  echo "  --- $name report"
  sed 's/^/  | /' "$OUT/$name.jsonl" || true
  echo "  --- $name log"
  sed 's/^/  | /' "$OUT/$name.log" || true
  [ "$code" -eq 0 ] && pass "the app exited 0" || fail "the app exited with $code"
}

echo "==> 1. the app with no backend"
run_app start '{"accounts":{"inject_record":"unused.jsonl"},"steps":[{"do":"wait_home","home":{"account":{"configured":false}}}],"quit":true}'
R="$OUT/start.jsonl"
L="$OUT/start.log"
check "$R" '.kind == "engine_info" and .found == true and (.path | test("tc-engine\\.exe$"))' "it found tc-engine.exe next to itself"
check "$R" '.kind == "engine_info" and .platform.os == "windows" and .platform.can_share == false' "it says it's Windows and can't share"
check "$R" '.kind == "script" and .done and .ok' "it says it isn't connected to a server, and the script passed"
logged "$L" "platform: windows (can share: false)" "the log says which platform"
logged "$L" "webview: " "the log names the WebView2 version"
logged "$L" "engine: found at" "the log says where the engine is"
logged "$L" "backend: none named" "the log says no backend is named"
logged "$L" "window Panel: made (hidden)" "the log says the panel was made"
logged "$L" "window Bar: made (hidden)" "the log says the call bar was made"
logged "$L" "window panel: made: tool window, no taskbar button, not in Alt-Tab, topmost" "the panel reads back as a topmost tool window"
logged "$L" "window bar: made: tool window, no taskbar button, not in Alt-Tab, topmost" "the bar reads back as a topmost tool window"
logged "$L" "exiting" "the log says it exited"
# The page inside a hidden window may not lay itself out; say so either way.
if grep -qF "window Panel: page drawn" "$L"; then
  pass "the panel's page loaded from the app itself and measured itself"
else
  echo "  note: the hidden panel's page didn't report its height (not a failure)"
fi

echo "==> 2. the panel and the bar shown unprompted"
run_app shown '{"accounts":{"inject_record":"unused.jsonl"},"windows":true,"steps":[{"do":"show","surface":"panel"},{"do":"wait_ms","ms":2500},{"do":"show","surface":"bar"},{"do":"wait_ms","ms":1500}],"quit":true}'
L="$OUT/shown.log"
check "$OUT/shown.jsonl" '.kind == "script" and .done and .ok' "the script passed"
logged "$L" "tray icon: made" "the tray icon was made"
logged "$L" "window panel: shown (script, without focus): tool window, no taskbar button, not in Alt-Tab, topmost, not focused" "the panel came up without taking the keyboard, still a tool window"
logged "$L" "window bar: shown (script, without focus): tool window, no taskbar button, not in Alt-Tab, topmost, not focused" "the bar came up without taking the keyboard, still a tool window"

if [ -n "$LIVEKIT" ]; then
  echo "==> 3. a guest joins an empty room on a LiveKit dev server"
  "$LIVEKIT" --dev --bind 127.0.0.1 >"$OUT/livekit.log" 2>&1 &
  lk=$!
  trap 'kill "$lk" 2>/dev/null || true' EXIT
  for _ in $(seq 1 60); do
    curl -fsS http://127.0.0.1:7880 >/dev/null 2>&1 && break
    sleep 1
  done
  curl -fsS http://127.0.0.1:7880 >/dev/null && pass "the LiveKit dev server is up" || fail "the LiveKit dev server didn't start"
  # The dev server's well-known key and secret (livekit-server --dev), not a secret.
  export TWINCURSOR_SERVER_URL=ws://127.0.0.1:7880 TWINCURSOR_API_KEY=devkey TWINCURSOR_API_SECRET=secret
  run_app join '{"call":{"room":"windows-smoke","name":"Windows guest","quality":"1080p","inject_record":"unused.jsonl","role":"join"},"steps":[{"do":"wait_for","event":{"event":"connection","state":"connected"}},{"do":"send","cmd":"leave"},{"do":"wait_exit"}],"quit":true}'
  R="$OUT/join.jsonl"
  check "$R" '.kind == "started" and .role == "join" and .args[0] == "view"' "the app started its engine as a viewer"
  check "$R" '.kind == "event" and .event.event == "hello" and .event.role == "guest"' "the engine said hello as a guest"
  check "$R" '.kind == "event" and .event.event == "connection" and .event.state == "connected"' "the engine connected to the room"
  check "$R" '.kind == "exited"' "the engine exited after leaving"
  check "$R" '.kind == "script" and .done and .ok' "every script step passed"
  logged "$OUT/join.log" "engine: started (Join" "the log says the engine started"
  logged "$OUT/join.log" "engine: in the app's job object (Windows ends it with the app)" "the engine reads back as inside the app's job object"
  kill "$lk" 2>/dev/null || true
fi

if [ "$failed" -ne 0 ]; then
  echo "==> the Windows smoke test FAILED (reports and logs in $OUT)" >&2
  exit 1
fi
echo "==> the Windows smoke test passed"
