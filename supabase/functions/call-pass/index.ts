// call-pass: POST { "call_id": "<uuid>" } with the user's access token, get back a LiveKit token
// for that call's room, if the database says this user may join (public.pass_claims).
//
// Environment: SUPABASE_URL and SUPABASE_ANON_KEY (set by Supabase), and LIVEKIT_URL,
// LIVEKIT_API_KEY and LIVEKIT_API_SECRET (set as function secrets). The API secret never leaves
// this function.

import { liveKitConfig, mintPass, parseCallId, parseClaims } from "./pass.ts";

function json(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json", "cache-control": "no-store" },
  });
}

const unauthorized = () => json(401, { error: "not_signed_in" });
const forbidden = () => json(403, { error: "not_allowed" });
const failed = () => json(500, { error: "server_error" });

async function handle(req: Request): Promise<Response> {
  if (req.method === "OPTIONS") return new Response(null, { status: 204 });
  if (req.method !== "POST") return json(405, { error: "method_not_allowed" });

  const supabaseUrl = Deno.env.get("SUPABASE_URL");
  const anonKey = Deno.env.get("SUPABASE_ANON_KEY");
  const livekit = liveKitConfig((name) => Deno.env.get(name));
  if (!supabaseUrl || !anonKey || Array.isArray(livekit)) {
    const missing = [
      ...(supabaseUrl ? [] : ["SUPABASE_URL"]),
      ...(anonKey ? [] : ["SUPABASE_ANON_KEY"]),
      ...(Array.isArray(livekit) ? livekit : []),
    ];
    console.error(`call-pass is not configured: missing ${missing.join(", ")}`);
    return failed();
  }

  const authorization = req.headers.get("authorization") ?? "";
  if (!/^Bearer \S+$/.test(authorization)) return unauthorized();
  // Calls made as the user, so the database's own rules apply.
  const headers = { apikey: anonKey, authorization };

  // The gateway checks the JWT's signature, but the project's anon key is a valid JWT too. Only a
  // real user session gets past this.
  const user = await fetch(`${supabaseUrl}/auth/v1/user`, { headers });
  await user.body?.cancel();
  if (user.status >= 400 && user.status < 500) return unauthorized();
  if (!user.ok) {
    console.error(`call-pass: auth check failed with HTTP ${user.status}`);
    return failed();
  }

  let body: unknown;
  try {
    body = await req.json();
  } catch {
    return json(400, { error: "bad_request" });
  }
  const callId = parseCallId(body);
  if (!callId) return json(400, { error: "bad_request" });

  const rpc = await fetch(`${supabaseUrl}/rest/v1/rpc/pass_claims`, {
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
}

Deno.serve(async (req) => {
  try {
    return await handle(req);
  } catch (e) {
    console.error(`call-pass: ${e instanceof Error ? e.name : "error"}`);
    return failed();
  }
});
