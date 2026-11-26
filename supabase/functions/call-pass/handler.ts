// The call-pass request handler, with its environment and `fetch` passed in so it can be tested
// without a server (pass_test.ts) and served outside the Edge Runtime (tests/call-pass.sh).
//
// Checks run in this order, so a caller's own mistakes are reported before the server's:
//   1. method          405 unless POST (OPTIONS gets 204)
//   2. a Bearer token  401 without one
//   3. the body        400 unless { "call_id": "<uuid>" }
//   4. who the user is 401 unless the token is a signed-in user's session, or if there's no
//                      public key to ask Auth with (the request's apikey, else
//                      SUPABASE_ANON_KEY); 500 only if SUPABASE_URL is missing
//   5. LiveKit config  500 if a LIVEKIT_* secret is missing (logged by name)
//   6. pass_claims     403 if the database says this user may not join

import { liveKitConfig, mintPass, parseCallId, parseClaims } from "./pass.ts";

export type Env = (name: string) => string | undefined;

function json(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json", "cache-control": "no-store" },
  });
}

const unauthorized = () => json(401, { error: "not_signed_in" });
const forbidden = () => json(403, { error: "not_allowed" });
const badRequest = () => json(400, { error: "bad_request" });
export const failed = () => json(500, { error: "server_error" });

export function makeHandler(env: Env, fetchFn: typeof fetch = fetch): (req: Request) => Promise<Response> {
  return async (req) => {
    if (req.method === "OPTIONS") return new Response(null, { status: 204 });
    if (req.method !== "POST") return json(405, { error: "method_not_allowed" });

    const authorization = req.headers.get("authorization") ?? "";
    if (!/^Bearer \S+$/.test(authorization)) return unauthorized();

    let body: unknown;
    try {
      body = await req.json();
    } catch {
      return badRequest();
    }
    const callId = parseCallId(body);
    if (!callId) return badRequest();

    const supabaseUrl = env("SUPABASE_URL");
    if (!supabaseUrl) {
      console.error("call-pass is not configured: missing SUPABASE_URL");
      return failed();
    }

    // The downstream calls use the public key the caller sent (the app puts its publishable or
    // anon key in `apikey`). SUPABASE_ANON_KEY is only a fallback: Supabase treats it as legacy,
    // and a project on the new publishable keys may not set it. Trusting the caller's key weakens
    // nothing: it is public, it only names the project's client to the gateway, and the requests
    // still go to our own SUPABASE_URL. What authorizes is the user's own JWT, which Auth checks
    // and the database's rules apply to. A wrong or made-up key only makes those calls fail,
    // which is the caller's mistake and answers 401.
    const apikey = req.headers.get("apikey") || env("SUPABASE_ANON_KEY");
    if (!apikey) return unauthorized();
    // Calls made as the user, so the database's own rules apply.
    const headers = { apikey, authorization };

    // The gateway checks the JWT's signature, but the project's anon key is a valid JWT too. Only
    // a real user session gets past this.
    const user = await fetchFn(`${supabaseUrl}/auth/v1/user`, { headers });
    await user.body?.cancel();
    if (user.status >= 400 && user.status < 500) return unauthorized();
    if (!user.ok) {
      console.error(`call-pass: auth check failed with HTTP ${user.status}`);
      return failed();
    }

    const livekit = liveKitConfig(env);
    if (Array.isArray(livekit)) {
      console.error(`call-pass is not configured: missing ${livekit.join(", ")}`);
      return failed();
    }

    const rpc = await fetchFn(`${supabaseUrl}/rest/v1/rpc/pass_claims`, {
      method: "POST",
      headers: { ...headers, "content-type": "application/json" },
      body: JSON.stringify({ call_id: callId }),
    });
    if (rpc.status === 401) {
      await rpc.body?.cancel();
      return unauthorized();
    }
    // pass_claims raises not_allowed (403) for anyone who may not join, whatever the reason.
    if (rpc.status === 400 || rpc.status === 403 || rpc.status === 404) {
      await rpc.body?.cancel();
      return forbidden();
    }
    if (!rpc.ok) {
      console.error(`call-pass: pass_claims failed with HTTP ${rpc.status}`);
      await rpc.body?.cancel();
      return failed();
    }
    const rows: unknown = await rpc.json();
    const claims = Array.isArray(rows) && rows.length === 1 ? parseClaims(rows[0]) : null;
    if (!claims) return forbidden();

    return json(200, await mintPass(claims, livekit));
  };
}

/** Wraps a handler so an unexpected error answers 500 without leaking its message. */
export function guarded(handle: (req: Request) => Promise<Response>): (req: Request) => Promise<Response> {
  return async (req) => {
    try {
      return await handle(req);
    } catch (e) {
      console.error(`call-pass: ${e instanceof Error ? e.name : "error"}`);
      return failed();
    }
  };
}
