#!/usr/bin/env bash
# End-to-end check of the call-pass Edge Function against the local stack.
#
# Needs a running local stack (`supabase start`). Serves the function with made-up LiveKit
# settings, runs supabase/tests/call-pass_e2e.ts, and stops serving. The local keys come from
# `supabase status` at run time and are never written to the repository.
#
# Besides the copy the stack serves, it runs two more with plain Deno and an environment that
# holds nothing else (`env -i`): one with SUPABASE_URL and the LiveKit settings but no
# SUPABASE_ANON_KEY, and one with SUPABASE_URL alone (LiveKit not configured).
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$root"

work="$(mktemp -d)"
pids=()
cleanup() {
  for pid in ${pids[@]+"${pids[@]}"}; do kill "$pid" 2>/dev/null || true; wait "$pid" 2>/dev/null || true; done
  rm -rf "$work"
}
trap cleanup EXIT

# Made-up LiveKit settings: the test only checks that tokens verify with this secret.
export TEST_LIVEKIT_URL="wss://livekit.example.test"
export TEST_LIVEKIT_API_KEY="APIe2etest"
TEST_LIVEKIT_API_SECRET="e2e-$(openssl rand -hex 24)"
export TEST_LIVEKIT_API_SECRET
cat > "$work/functions.env" <<EOF
LIVEKIT_URL=$TEST_LIVEKIT_URL
LIVEKIT_API_KEY=$TEST_LIVEKIT_API_KEY
LIVEKIT_API_SECRET=$TEST_LIVEKIT_API_SECRET
EOF

eval "$(supabase status -o env 2>/dev/null | grep -E '^(API_URL|ANON_KEY|SERVICE_ROLE_KEY|PUBLISHABLE_KEY)=')"
export API_URL ANON_KEY SERVICE_ROLE_KEY
# The new-style public key, where the CLI has one (the e2e skips that step otherwise).
export PUBLISHABLE_KEY="${PUBLISHABLE_KEY:-}"
if [[ -z "${API_URL:-}" ]]; then
  echo "The local stack isn't running: run 'supabase start' first." >&2
  exit 1
fi

supabase functions serve --env-file "$work/functions.env" > "$work/serve.log" 2>&1 &
pids+=($!)

# The bare copies: ports next to the stack's (54420-54423).
export BARE_URL="http://127.0.0.1:54426" BARE_UNCONFIGURED_URL="http://127.0.0.1:54427"
bare() { # PORT LOG [NAME=VALUE ...]
  local port="$1" log="$2"
  shift 2
  env -i PATH="$PATH" HOME="$HOME" ${DENO_DIR:+DENO_DIR="$DENO_DIR"} SUPABASE_URL="$API_URL" "$@" \
    deno run --config supabase/functions/call-pass/deno.json --allow-net=127.0.0.1 --allow-env \
    supabase/tests/call-pass_serve.ts "$port" > "$log" 2>&1 &
  pids+=($!)
}
bare 54426 "$work/bare.log" LIVEKIT_URL="$TEST_LIVEKIT_URL" LIVEKIT_API_KEY="$TEST_LIVEKIT_API_KEY" \
  LIVEKIT_API_SECRET="$TEST_LIVEKIT_API_SECRET"
bare 54427 "$work/bare-unconfigured.log"

# Wait until each copy answers (a request without a session gets 401 once it's up).
wait_for() { # URL LOG
  local status=""
  for _ in $(seq 1 60); do
    status="$(curl -s -o /dev/null -w '%{http_code}' -X POST "$1" \
      -H "apikey: $ANON_KEY" -H 'content-type: application/json' -d '{}' || true)"
    if [[ "$status" == "401" ]]; then return 0; fi
    sleep 1
  done
  echo "call-pass at $1 didn't come up (last HTTP status: ${status:-none}). Log:" >&2
  cat "$2" >&2
  exit 1
}
wait_for "$API_URL/functions/v1/call-pass" "$work/serve.log"
wait_for "$BARE_URL" "$work/bare.log"
wait_for "$BARE_UNCONFIGURED_URL" "$work/bare-unconfigured.log"

deno test --config supabase/functions/call-pass/deno.json \
  --allow-env --allow-net=127.0.0.1,localhost \
  supabase/tests/call-pass_e2e.ts

# The unconfigured copy must have said what's missing, by name only.
grep -q "missing LIVEKIT_URL, LIVEKIT_API_KEY, LIVEKIT_API_SECRET" "$work/bare-unconfigured.log" || {
  echo "the unconfigured copy didn't log the missing LiveKit settings. Log:" >&2
  cat "$work/bare-unconfigured.log" >&2
  exit 1
}
