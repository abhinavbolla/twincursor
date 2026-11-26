#!/usr/bin/env bash
# End-to-end check of the call-pass Edge Function against the local stack.
#
# Needs a running local stack (`supabase start`). Serves the function with made-up LiveKit
# settings, runs supabase/tests/call-pass_e2e.ts, and stops serving. The local keys come from
# `supabase status` at run time and are never written to the repository.
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$root"

work="$(mktemp -d)"
serve_pid=""
cleanup() {
  if [[ -n "$serve_pid" ]]; then kill "$serve_pid" 2>/dev/null || true; wait "$serve_pid" 2>/dev/null || true; fi
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

eval "$(supabase status -o env 2>/dev/null | grep -E '^(API_URL|ANON_KEY|SERVICE_ROLE_KEY)=')"
export API_URL ANON_KEY SERVICE_ROLE_KEY
if [[ -z "${API_URL:-}" ]]; then
  echo "The local stack isn't running: run 'supabase start' first." >&2
  exit 1
fi

supabase functions serve --env-file "$work/functions.env" > "$work/serve.log" 2>&1 &
serve_pid=$!

# Wait until the function answers (a request without a session gets 401 once it's up).
for _ in $(seq 1 60); do
  status="$(curl -s -o /dev/null -w '%{http_code}' -X POST "$API_URL/functions/v1/call-pass" \
    -H "apikey: $ANON_KEY" -H 'content-type: application/json' -d '{}' || true)"
  if [[ "$status" == "401" ]]; then break; fi
  sleep 1
done
if [[ "${status:-}" != "401" ]]; then
  echo "call-pass didn't come up (last HTTP status: ${status:-none}). Serve log:" >&2
  cat "$work/serve.log" >&2
  exit 1
fi

deno test --config supabase/functions/call-pass/deno.json \
  --allow-env --allow-net=127.0.0.1,localhost \
  supabase/tests/call-pass_e2e.ts
