// End-to-end check of the call-pass function against the local stack. Run it with
// supabase/tests/call-pass.sh, which serves the function with made-up LiveKit settings and passes
// the local keys in the environment.

import { assert, assertEquals } from "@std/assert";
import { jwtVerify } from "jose";

const env = (name: string): string => {
  const v = Deno.env.get(name);
  if (!v) throw new Error(`${name} is not set; run this through supabase/tests/call-pass.sh`);
  return v;
};
const api = env("API_URL");
const anonKey = env("ANON_KEY");
const serviceKey = env("SERVICE_ROLE_KEY");
const livekitKey = env("TEST_LIVEKIT_API_KEY");
const livekitSecret = new TextEncoder().encode(env("TEST_LIVEKIT_API_SECRET"));
const livekitUrl = env("TEST_LIVEKIT_URL");

const run = crypto.randomUUID().slice(0, 8);
const created: string[] = [];

async function call(path: string, init: RequestInit & { token?: string; key?: string } = {}) {
  const headers = new Headers(init.headers);
  headers.set("apikey", init.key ?? anonKey);
  if (init.token) headers.set("authorization", `Bearer ${init.token}`);
  if (init.body) headers.set("content-type", "application/json");
  const res = await fetch(`${api}${path}`, { ...init, headers });
  const text = await res.text();
  return { status: res.status, body: text ? JSON.parse(text) : null };
}

/** Creates a confirmed password user through the admin API and signs them in. */
async function user(name: string): Promise<{ id: string; token: string }> {
  const email = `${name.toLowerCase()}-${run}@example.test`;
  const password = crypto.randomUUID();
  const made = await call("/auth/v1/admin/users", {
    method: "POST",
    key: serviceKey,
    token: serviceKey,
    body: JSON.stringify({ email, password, email_confirm: true, user_metadata: { name } }),
  });
  assertEquals(made.status, 200, JSON.stringify(made.body));
  created.push(made.body.id);
  const session = await call("/auth/v1/token?grant_type=password", {
    method: "POST",
    body: JSON.stringify({ email, password }),
  });
  assertEquals(session.status, 200, JSON.stringify(session.body));
  return { id: made.body.id, token: session.body.access_token };
}

const rpc = (fn: string, token: string, args: unknown = {}) =>
  call(`/rest/v1/rpc/${fn}`, { method: "POST", token, body: JSON.stringify(args) });

const pass = (token: string | undefined, body: unknown) =>
  call("/functions/v1/call-pass", { method: "POST", token, body: JSON.stringify(body) });

async function claims(token: string) {
  const { payload } = await jwtVerify(token, livekitSecret, { issuer: livekitKey, algorithms: ["HS256"] });
  return payload as Record<string, unknown> & { video: Record<string, unknown> };
}

Deno.test("call-pass gives passes only to the host and admitted members", async (t) => {
  const host = await user("Hana");
  const knocker = await user("Kai");
  const stranger = await user("Sam");
  try {
    const started = await rpc("start_call", host.token);
    assertEquals(started.status, 200, JSON.stringify(started.body));
    const { call_id, code } = started.body[0];

    await t.step("inbox works with and without the hosting argument", async () => {
      const plain = await rpc("inbox", host.token);
      assertEquals(plain.status, 200, JSON.stringify(plain.body));
      assertEquals(plain.body.hosting.call_id, call_id);
      const beat = await rpc("inbox", host.token, { hosting: call_id });
      assertEquals(beat.status, 200, JSON.stringify(beat.body));
      assertEquals(beat.body.hosting.code, code);
    });

    await t.step("rejects requests without a user session", async () => {
      assertEquals((await pass(undefined, { call_id })).status, 401);
      assertEquals((await pass(anonKey, { call_id })).status, 401);
      assertEquals((await pass("not-a-jwt", { call_id })).status, 401);
    });

    await t.step("rejects a bad body", async () => {
      assertEquals((await pass(host.token, { call: call_id })).status, 400);
      assertEquals((await pass(host.token, { call_id: "nope" })).status, 400);
      const raw = await call("/functions/v1/call-pass", { method: "POST", token: host.token, body: "{" });
      assertEquals(raw.status, 400);
    });

    await t.step("a knocker gets 403 until let in", async () => {
      const knock = await rpc("knock", knocker.token, { code });
      assertEquals(knock.status, 200, JSON.stringify(knock.body));
      assertEquals(knock.body[0].state, "knocking");
      const refused = await pass(knocker.token, { call_id });
      assertEquals(refused.status, 403);
      assertEquals(refused.body, { error: "not_allowed" });
    });

    await t.step("a stranger and an unknown call get 403", async () => {
      assertEquals((await pass(stranger.token, { call_id })).status, 403);
      assertEquals((await pass(host.token, { call_id: crypto.randomUUID() })).status, 403);
    });

    await t.step("the host gets a sharer pass", async () => {
      const res = await pass(host.token, { call_id });
      assertEquals(res.status, 200, JSON.stringify(res.body));
      assertEquals(res.body.role, "sharer");
      assertEquals(res.body.identity, `sharer-${host.id}`);
      assertEquals(res.body.name, "Hana");
      assertEquals(res.body.url, livekitUrl);
      const c = await claims(res.body.token);
      assertEquals(c.sub, `sharer-${host.id}`);
      assertEquals(c.name, "Hana");
      assertEquals(c.video.room, res.body.room);
      assertEquals(c.video.canPublishSources, undefined);
      assertEquals((c.exp as number) - (c.nbf as number), 600);
    });

    await t.step("once admitted, the knocker gets a guest pass", async () => {
      const admit = await rpc("answer_knock", host.token, { call_id, user_id: knocker.id, let_in: true });
      assertEquals(admit.status, 200, JSON.stringify(admit.body));
      const res = await pass(knocker.token, { call_id });
      assertEquals(res.status, 200, JSON.stringify(res.body));
      assertEquals(Object.keys(res.body).sort(), ["identity", "name", "role", "room", "token", "url"]);
      assertEquals(res.body.role, "guest");
      assertEquals(res.body.identity, `guest-${knocker.id}`);
      const c = await claims(res.body.token);
      assertEquals(c.sub, `guest-${knocker.id}`);
      assertEquals(c.name, "Kai");
      assertEquals(c.video, {
        room: res.body.room,
        roomJoin: true,
        canSubscribe: true,
        canPublish: true,
        canPublishData: true,
        canPublishSources: ["microphone", "camera"],
      });
      assert(/^[0-9a-f]{32}$/.test(res.body.room));
    });

    await t.step("the stranger still gets 403, and nothing after the call ends", async () => {
      assertEquals((await pass(stranger.token, { call_id })).status, 403);
      assertEquals((await rpc("end_call", host.token, { call_id })).status, 204);
      assertEquals((await pass(host.token, { call_id })).status, 403);
      assertEquals((await pass(knocker.token, { call_id })).status, 403);
    });

    await t.step("anon cannot call the RPCs directly", async () => {
      const res = await call("/rest/v1/rpc/inbox", { method: "POST", token: anonKey, body: "{}" });
      assert(res.status === 401 || res.status === 403 || res.status === 404, `HTTP ${res.status}`);
    });
  } finally {
    for (const id of created) {
      await call(`/auth/v1/admin/users/${id}`, { method: "DELETE", key: serviceKey, token: serviceKey });
    }
  }
});
