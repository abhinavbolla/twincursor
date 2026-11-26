import { assert, assertEquals, assertRejects } from "@std/assert";
import { decodeProtectedHeader, jwtVerify } from "jose";
import { type Env, makeHandler } from "./handler.ts";
import {
  liveKitConfig,
  mintPass,
  parseCallId,
  parseClaims,
  PASS_TTL_SECONDS,
  type PassClaims,
} from "./pass.ts";

// Made-up values, only for these tests.
const config = {
  url: "wss://livekit.example.test",
  apiKey: "APItest",
  apiSecret: "test-secret-0123456789abcdef0123456789",
};
const secret = new TextEncoder().encode(config.apiSecret);

const sharer: PassClaims = {
  room: "0123456789abcdef0123456789abcdef",
  identity: "sharer-7d1c3f0e-0000-4000-8000-000000000001",
  name: "Ada",
  role: "sharer",
};
const guest: PassClaims = {
  room: sharer.room,
  identity: "guest-7d1c3f0e-0000-4000-8000-000000000002",
  name: "Grace",
  role: "guest",
};

async function verified(token: string) {
  const { payload } = await jwtVerify(token, secret, { issuer: config.apiKey, algorithms: ["HS256"] });
  return payload as Record<string, unknown> & { video: Record<string, unknown> };
}

Deno.test("a sharer pass verifies and carries the sharer's identity, name and room", async () => {
  const before = Math.floor(Date.now() / 1000);
  const pass = await mintPass(sharer, config);
  assertEquals(decodeProtectedHeader(pass.token).alg, "HS256");
  const claims = await verified(pass.token);

  assertEquals(claims.iss, config.apiKey);
  assertEquals(claims.sub, sharer.identity);
  assertEquals(claims.name, sharer.name);
  assertEquals(claims.video, {
    room: sharer.room,
    roomJoin: true,
    canSubscribe: true,
    canPublish: true,
    canPublishData: true,
  });
  assertEquals(pass.url, config.url);
  assertEquals(pass.room, sharer.room);
  assertEquals(pass.identity, sharer.identity);
  assertEquals(pass.role, "sharer");

  const nbf = claims.nbf as number;
  const exp = claims.exp as number;
  assert(nbf >= before - 1 && nbf <= before + 5, `nbf ${nbf}`);
  assertEquals(exp - nbf, PASS_TTL_SECONDS);
  assertEquals(PASS_TTL_SECONDS, 600);
});

Deno.test("a guest pass may publish only the microphone and camera", async () => {
  const pass = await mintPass(guest, config);
  const claims = await verified(pass.token);
  assertEquals(claims.sub, guest.identity);
  assertEquals(claims.name, guest.name);
  assertEquals(claims.video, {
    room: guest.room,
    roomJoin: true,
    canSubscribe: true,
    canPublish: true,
    canPublishData: true,
    canPublishSources: ["microphone", "camera"],
  });
  assertEquals(pass.role, "guest");
});

Deno.test("a pass doesn't verify with another secret", async () => {
  const pass = await mintPass(sharer, config);
  await assertRejects(() =>
    jwtVerify(pass.token, new TextEncoder().encode("another-secret-0123456789abcdef"))
  );
});

Deno.test("parseCallId accepts only a uuid call_id", () => {
  const id = "7D1C3F0E-0000-4000-8000-000000000001";
  assertEquals(parseCallId({ call_id: id }), id.toLowerCase());
  const bad = [null, "x", [], {}, { call_id: 1 }, { call_id: "x" }, { call_id: `${id} ` }, [{ call_id: id }]];
  for (const body of bad) assertEquals(parseCallId(body), null, JSON.stringify(body));
});

Deno.test("parseClaims rejects rows that don't match their role", () => {
  assertEquals(parseClaims(sharer), sharer);
  assertEquals(parseClaims(guest), guest);
  assertEquals(parseClaims({ ...guest, role: "sharer" }), null);
  assertEquals(parseClaims({ ...sharer, role: "admin" }), null);
  assertEquals(parseClaims({ ...sharer, room: "" }), null);
  assertEquals(parseClaims(null), null);
});

Deno.test("liveKitConfig names what's missing, never the values", () => {
  const env: Record<string, string> = { LIVEKIT_URL: "wss://x.test", LIVEKIT_API_SECRET: "s" };
  assertEquals(liveKitConfig((n) => env[n]), ["LIVEKIT_API_KEY"]);
  env.LIVEKIT_API_KEY = "k";
  assertEquals(liveKitConfig((n) => env[n]), { url: "wss://x.test", apiKey: "k", apiSecret: "s" });
});

// The handler, with a made-up environment and a stand-in for Auth and the database.

const callId = "7d1c3f0e-0000-4000-8000-00000000c411";
const userToken = "user-session-jwt";
const goodKey = "sb_publishable_test";

/** Answers like the gateway: 401 for a wrong key or a token that isn't a user session. */
function backend(seen: { url: string; apikey: string | null }[] = []): typeof fetch {
  return (input, init) => {
    const url = String(input);
    const h = new Headers(init?.headers);
    seen.push({ url, apikey: h.get("apikey") });
    const reply = (status: number, body: unknown = {}) =>
      Promise.resolve(new Response(JSON.stringify(body), { status }));
    if (h.get("apikey") !== goodKey) return reply(401, { message: "Invalid API key" });
    if (h.get("authorization") !== `Bearer ${userToken}`) return reply(401);
    if (url.endsWith("/auth/v1/user")) return reply(200, { id: "u" });
    if (url.endsWith("/rest/v1/rpc/pass_claims")) return reply(200, [sharer]);
    return reply(404);
  };
}

const configured: Record<string, string> = {
  SUPABASE_URL: "http://supabase.example.test",
  LIVEKIT_URL: config.url,
  LIVEKIT_API_KEY: config.apiKey,
  LIVEKIT_API_SECRET: config.apiSecret,
};
const envOf = (vars: Record<string, string>): Env => (name) => vars[name];

function request(
  opts: { method?: string; token?: string; apikey?: string; body?: string } = {},
): Request {
  const headers = new Headers();
  if (opts.token) headers.set("authorization", `Bearer ${opts.token}`);
  if (opts.apikey) headers.set("apikey", opts.apikey);
  const method = opts.method ?? "POST";
  return new Request("http://fn.test/call-pass", {
    method,
    headers,
    body: method === "POST" ? (opts.body ?? JSON.stringify({ call_id: callId })) : undefined,
  });
}

/** Runs `fn` with console.error captured, and returns what was logged. */
async function quietly(fn: () => Promise<void>): Promise<string[]> {
  const logged: string[] = [];
  const original = console.error;
  console.error = (...args: unknown[]) => logged.push(args.join(" "));
  try {
    await fn();
  } finally {
    console.error = original;
  }
  return logged;
}

Deno.test("without SUPABASE_ANON_KEY the handler uses the request's apikey", async () => {
  assertEquals(configured.SUPABASE_ANON_KEY, undefined);
  const seen: { url: string; apikey: string | null }[] = [];
  const handle = makeHandler(envOf(configured), backend(seen));
  const res = await handle(request({ token: userToken, apikey: goodKey }));
  assertEquals(res.status, 200);
  const body = await res.json();
  assertEquals(body.identity, sharer.identity);
  assertEquals((await verified(body.token)).sub, sharer.identity);
  assertEquals(seen.map((s) => s.apikey), [goodKey, goodKey]);
});

Deno.test("SUPABASE_ANON_KEY is the fallback only when the request has no apikey", async () => {
  const seen: { url: string; apikey: string | null }[] = [];
  const handle = makeHandler(envOf({ ...configured, SUPABASE_ANON_KEY: goodKey }), backend(seen));
  assertEquals((await handle(request({ token: userToken }))).status, 200);
  assertEquals(seen.map((s) => s.apikey), [goodKey, goodKey]);
  // A key the caller sent wins over the environment's, even when it's wrong.
  const wrong = await handle(request({ token: userToken, apikey: "sb_publishable_wrong" }));
  assertEquals(wrong.status, 401);
  await wrong.body?.cancel();
});

Deno.test("no apikey and no SUPABASE_ANON_KEY is 401, without asking Auth", async () => {
  const seen: { url: string; apikey: string | null }[] = [];
  const res = await makeHandler(envOf(configured), backend(seen))(request({ token: userToken }));
  assertEquals(res.status, 401);
  assertEquals(await res.json(), { error: "not_signed_in" });
  assertEquals(seen, []);
});

Deno.test("a wrong apikey is the caller's mistake: 401, not 500", async () => {
  const res = await makeHandler(envOf(configured), backend())(
    request({ token: userToken, apikey: "made-up" }),
  );
  assertEquals(res.status, 401);
  await res.body?.cancel();
});

Deno.test("the caller's mistakes come before the server's configuration", async () => {
  // Nothing configured at all: not even SUPABASE_URL.
  const handle = makeHandler(envOf({}), backend());
  const statuses = async (req: Request) => {
    const res = await handle(req);
    await res.body?.cancel();
    return res.status;
  };
  assertEquals(await statuses(request({ method: "GET" })), 405);
  assertEquals((await handle(request({ method: "OPTIONS" }))).status, 204);
  assertEquals(await statuses(request({ apikey: goodKey })), 401);
  assertEquals(await statuses(request({ token: userToken, apikey: goodKey, body: "{" })), 400);
  assertEquals(await statuses(request({ token: userToken, apikey: goodKey, body: '{"call":1}' })), 400);
});

Deno.test("LiveKit not configured: 500 for a signed-in user, 401 for anyone else", async () => {
  const supabaseOnly = { SUPABASE_URL: configured.SUPABASE_URL, LIVEKIT_URL: config.url };
  const handle = makeHandler(envOf(supabaseOnly), backend());
  const logged = await quietly(async () => {
    const user = await handle(request({ token: userToken, apikey: goodKey }));
    assertEquals(user.status, 500);
    assertEquals(await user.json(), { error: "server_error" });
    const anon = await handle(request({ token: "anon-key-jwt", apikey: goodKey }));
    assertEquals(anon.status, 401);
    await anon.body?.cancel();
    const none = await handle(request({ apikey: goodKey }));
    assertEquals(none.status, 401);
    await none.body?.cancel();
  });
  assertEquals(logged, ["call-pass is not configured: missing LIVEKIT_API_KEY, LIVEKIT_API_SECRET"]);
});
