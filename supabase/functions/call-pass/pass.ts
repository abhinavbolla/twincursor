// Turns the database's answer to "may this person join?" into a LiveKit access token.
// No I/O here, so it can be tested on its own (pass_test.ts).

import { AccessToken, TrackSource } from "livekit-server-sdk";

/** How long a call pass is valid for joining. LiveKit refreshes the token once connected. */
export const PASS_TTL_SECONDS = 10 * 60;

export type Role = "sharer" | "guest";

/** One row from `public.pass_claims(call_id)`. */
export interface PassClaims {
  room: string;
  identity: string;
  name: string;
  role: Role;
}

export interface LiveKitConfig {
  url: string;
  apiKey: string;
  apiSecret: string;
}

/** What the function returns to the app. */
export interface CallPass extends PassClaims {
  url: string;
  token: string;
}

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

/** The call id from a request body, or null if the body isn't `{ "call_id": "<uuid>" }`. */
export function parseCallId(body: unknown): string | null {
  if (typeof body !== "object" || body === null || Array.isArray(body)) return null;
  const id = (body as Record<string, unknown>).call_id;
  return typeof id === "string" && UUID.test(id) ? id.toLowerCase() : null;
}

/** Checks a row from `pass_claims` before anything is signed with it. */
export function parseClaims(row: unknown): PassClaims | null {
  if (typeof row !== "object" || row === null) return null;
  const r = row as Record<string, unknown>;
  const { room, identity, name, role } = r;
  if (typeof room !== "string" || room === "") return null;
  if (typeof identity !== "string" || typeof name !== "string") return null;
  if (role !== "sharer" && role !== "guest") return null;
  if (!identity.startsWith(`${role}-`)) return null;
  return { room, identity, name, role };
}

/**
 * Signs a call pass. Both roles may join, subscribe, publish and send data. A guest may only
 * publish their microphone and camera, so a guest can never publish a screen share.
 */
export async function mintPass(claims: PassClaims, config: LiveKitConfig): Promise<CallPass> {
  const at = new AccessToken(config.apiKey, config.apiSecret, {
    identity: claims.identity,
    name: claims.name,
    ttl: PASS_TTL_SECONDS,
  });
  at.addGrant({
    room: claims.room,
    roomJoin: true,
    canSubscribe: true,
    canPublish: true,
    canPublishData: true,
    ...(claims.role === "guest" ? { canPublishSources: [TrackSource.MICROPHONE, TrackSource.CAMERA] } : {}),
  });
  return { url: config.url, token: await at.toJwt(), ...claims };
}

/** LiveKit settings from the function's environment, or the names of the ones missing. */
export function liveKitConfig(env: (name: string) => string | undefined): LiveKitConfig | string[] {
  const url = env("LIVEKIT_URL");
  const apiKey = env("LIVEKIT_API_KEY");
  const apiSecret = env("LIVEKIT_API_SECRET");
  if (url && apiKey && apiSecret) return { url, apiKey, apiSecret };
  return [
    ...(url ? [] : ["LIVEKIT_URL"]),
    ...(apiKey ? [] : ["LIVEKIT_API_KEY"]),
    ...(apiSecret ? [] : ["LIVEKIT_API_SECRET"]),
  ];
}
