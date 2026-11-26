// The engine's protocol (crates/ipc, docs/ipc.md) as TypeScript, and what the app's Rust side
// sends the window about a call (`Output` in src-tauri/src/engine.rs).

/** The protocol version this window understands (`ipc::VERSION`). */
export const PROTOCOL_VERSION = 1;

export type Role = "sharer" | "guest" | "unknown";
export type ConnectionState = "connecting" | "connected" | "reconnecting" | "disconnected";
export type ControlState = "none" | "asked" | "controlling";
export type EndReason = "left" | "disconnected" | "failed";

export interface Participant {
  identity: string;
  /** What they typed. Not proof of who they are. */
  name: string;
  role: Role;
  /** Guests: the codecs they decode. Empty until they say. */
  decoders: string[];
}

export interface Peer {
  identity: string;
  /** The name in their token: with a call pass, their account's name. */
  name: string;
}

export interface Stats {
  width: number;
  height: number;
  fps: number;
  kbps: number;
  rtt_ms: number | null;
  codec: string | null;
  shown_pct: number | null;
}

export interface Voice {
  sending: boolean;
  muted: boolean;
  mic: string | null;
  speaker: string | null;
}

export interface ControlAsked {
  request: number;
  identity: string;
  name: string;
  platform: string;
}

export type EngineEvent =
  | { event: "hello"; version: number; role: Role; engine: string }
  | { event: "connection"; state: ConnectionState }
  | { event: "participants"; participants: Participant[] }
  | ({ event: "control_asked" } & ControlAsked)
  | {
      event: "control";
      state: ControlState;
      by: Peer | null;
      /** Sharer: guests who may take control without asking, for this call. Always empty on a guest. */
      trusted: Peer[];
    }
  | ({ event: "voice" } & Voice)
  | ({ event: "stats" } & Stats)
  | { event: "error"; message: string }
  | { event: "ended"; reason: EndReason };

export type EngineCommand =
  | { cmd: "mute" }
  | { cmd: "unmute" }
  | { cmd: "leave" }
  | { cmd: "allow_control"; request: number }
  | { cmd: "deny_control"; request: number }
  | { cmd: "revoke_control" }
  | { cmd: "trust_control"; identity: string }
  | { cmd: "trust_on_arrival"; identity: string }
  | { cmd: "request_control" }
  | { cmd: "release_control" }
  | { cmd: "set_mic"; device: string | null }
  | { cmd: "set_speaker"; device: string | null };

/** How the person set up the call (`Plan` in engine.rs). */
export type CallRole = "share" | "join";
export type Quality = "1080p" | "1440p" | "4k";
export type Codec = "h264" | "h265";

export interface Plan {
  role: CallRole;
  room: string;
  name: string;
  quality: Quality;
  voice: boolean;
}

/** What the Rust side tells the window about the call. */
export type EngineOutput =
  | { kind: "started"; role: CallRole; codec: Codec; pid: number; args: string[] }
  | { kind: "event"; event: unknown }
  | { kind: "log"; line: string }
  | { kind: "restarting"; codec: Codec; message: string }
  | { kind: "exited"; code: number | null };

type Obj = Record<string, unknown>;

const isObj = (v: unknown): v is Obj => typeof v === "object" && v !== null && !Array.isArray(v);
const isStr = (v: unknown): v is string => typeof v === "string";
const isNum = (v: unknown): v is number => typeof v === "number" && Number.isFinite(v);
const isBool = (v: unknown): v is boolean => typeof v === "boolean";
const orNull =
  <T>(check: (v: unknown) => v is T) =>
  (v: unknown): v is T | null =>
    v === null || check(v);
const oneOf =
  <T extends string>(...values: T[]) =>
  (v: unknown): v is T =>
    isStr(v) && (values as string[]).includes(v);

const isRole = (v: unknown): v is Role => isStr(v);
const isPeer = (v: unknown): v is Peer => isObj(v) && isStr(v.identity) && isStr(v.name);
const isParticipant = (v: unknown): v is Participant =>
  isObj(v) &&
  isStr(v.identity) &&
  isStr(v.name) &&
  isRole(v.role) &&
  Array.isArray(v.decoders) &&
  v.decoders.every(isStr);

/** Reads one engine event, or null if it isn't one this window knows. */
export function parseEvent(value: unknown): EngineEvent | null {
  if (!isObj(value) || !isStr(value.event)) {
    return null;
  }
  const v = value;
  switch (v.event) {
    case "hello":
      return isNum(v.version) && isRole(v.role) && isStr(v.engine)
        ? {
            event: "hello",
            version: v.version,
            role: oneOf("sharer", "guest")(v.role) ? v.role : "unknown",
            engine: v.engine,
          }
        : null;
    case "connection":
      return oneOf("connecting", "connected", "reconnecting", "disconnected")(v.state)
        ? { event: "connection", state: v.state }
        : null;
    case "participants":
      return Array.isArray(v.participants) && v.participants.every(isParticipant)
        ? {
            event: "participants",
            participants: v.participants.map((p) => ({
              ...p,
              role: oneOf("sharer", "guest")(p.role) ? p.role : "unknown",
            })),
          }
        : null;
    case "control_asked":
      return isNum(v.request) && isStr(v.identity) && isStr(v.name) && isStr(v.platform)
        ? {
            event: "control_asked",
            request: v.request,
            identity: v.identity,
            name: v.name,
            platform: v.platform,
          }
        : null;
    case "control": {
      // An engine from before trusted control leaves `trusted` out: nobody is trusted.
      const trusted: unknown = v.trusted === undefined ? [] : v.trusted;
      return oneOf("none", "asked", "controlling")(v.state) &&
        orNull(isPeer)(v.by) &&
        Array.isArray(trusted) &&
        trusted.every(isPeer)
        ? { event: "control", state: v.state, by: v.by, trusted }
        : null;
    }
    case "voice":
      return isBool(v.sending) && isBool(v.muted) && orNull(isStr)(v.mic) && orNull(isStr)(v.speaker)
        ? { event: "voice", sending: v.sending, muted: v.muted, mic: v.mic, speaker: v.speaker }
        : null;
    case "stats":
      return isNum(v.width) &&
        isNum(v.height) &&
        isNum(v.fps) &&
        isNum(v.kbps) &&
        orNull(isNum)(v.rtt_ms) &&
        orNull(isStr)(v.codec) &&
        orNull(isNum)(v.shown_pct)
        ? {
            event: "stats",
            width: v.width,
            height: v.height,
            fps: v.fps,
            kbps: v.kbps,
            rtt_ms: v.rtt_ms,
            codec: v.codec,
            shown_pct: v.shown_pct,
          }
        : null;
    case "error":
      return isStr(v.message) ? { event: "error", message: v.message } : null;
    case "ended":
      return oneOf("left", "disconnected", "failed")(v.reason)
        ? { event: "ended", reason: v.reason }
        : null;
    default:
      return null;
  }
}

/** Reads one output from the Rust side, or null. */
export function parseOutput(value: unknown): EngineOutput | null {
  if (!isObj(value)) {
    return null;
  }
  const v = value;
  const isCodec = oneOf("h264", "h265");
  switch (v.kind) {
    case "started":
      return oneOf("share", "join")(v.role) &&
        isCodec(v.codec) &&
        isNum(v.pid) &&
        Array.isArray(v.args) &&
        v.args.every(isStr)
        ? { kind: "started", role: v.role, codec: v.codec, pid: v.pid, args: v.args }
        : null;
    case "event":
      return { kind: "event", event: v.event };
    case "log":
      return isStr(v.line) ? { kind: "log", line: v.line } : null;
    case "restarting":
      return isCodec(v.codec) && isStr(v.message)
        ? { kind: "restarting", codec: v.codec, message: v.message }
        : null;
    case "exited":
      return orNull(isNum)(v.code) ? { kind: "exited", code: v.code } : null;
    default:
      return null;
  }
}
