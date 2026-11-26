import { assert, assertEquals, assertRejects } from "@std/assert";
import { decodeProtectedHeader, jwtVerify } from "jose";
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
