// The call as the window shows it: a pure reducer from what the engine says (via the Rust side)
// to state, so every event can be tested without the app.

import {
  parseEvent,
  PROTOCOL_VERSION,
  type CallRole,
  type Codec,
  type ConnectionState,
  type ControlAsked,
  type ControlState,
  type EndReason,
  type EngineOutput,
  type Participant,
  type Peer,
  type Stats,
  type Voice,
} from "./ipc";

/** Engine log lines kept for the Details area. */
export const MAX_LOGS = 300;

export interface CallState {
  phase: "idle" | "in_call";
  role: CallRole | null;
  codec: Codec | null;
  /** The engine's version, from its hello. */
  engine: string | null;
  /** The flags the current engine was started with (never the secret). */
  args: string[];
  connection: ConnectionState | null;
  participants: Participant[];
  control: { state: ControlState; by: Peer | null };
  /** Sharer: a request waiting for Allow or Deny. */
  asked: ControlAsked | null;
  voice: Voice | null;
  stats: Stats | null;
  /** The latest error, as a sentence. */
  error: string | null;
  /** Something the app did on its own, such as switching to H.264. */
  notice: string | null;
  ended: EndReason | null;
  /** Why the last call ended, shown on the start screen. */
  endMessage: string | null;
  logs: string[];
}

export type CallAction =
  | { type: "output"; output: EngineOutput }
  /** The sharer answered the request on screen. */
  | { type: "answered" }
  /** Starting failed before an engine ran. */
  | { type: "start_failed"; message: string }
  | { type: "dismiss" };

const noControl = { state: "none" as const, by: null };

export const initialCall: CallState = {
  phase: "idle",
  role: null,
  codec: null,
  engine: null,
  args: [],
  connection: null,
  participants: [],
  control: noControl,
  asked: null,
  voice: null,
  stats: null,
  error: null,
  notice: null,
  ended: null,
  endMessage: null,
  logs: [],
};

/** What a fresh engine knows nothing of yet. */
const engineReset = {
  connection: null,
  participants: [],
  control: noControl,
  asked: null,
  voice: null,
  stats: null,
  ended: null,
} satisfies Partial<CallState>;

const addLog = (logs: string[], line: string): string[] => [...logs, line].slice(-MAX_LOGS);

/** Why the call ended, in a sentence. */
export function endMessage(ended: EndReason | null, error: string | null, code: number | null): string {
  switch (ended) {
    case "left":
      return "You left the call.";
    case "disconnected":
      return "The call ended. The connection to the server closed.";
    case "failed":
      return `The call stopped: ${error ?? "the engine failed."}`;
    case null:
      if (code === 0) {
        return "The call ended.";
      }
      return `The engine stopped unexpectedly${code === null ? "" : ` (exit code ${code})`}${
        error === null ? "." : `: ${error}`
      }`;
  }
}

function onEvent(state: CallState, raw: unknown): CallState {
  const event = parseEvent(raw);
  if (event === null) {
    return { ...state, logs: addLog(state.logs, `(an event this window can't read) ${JSON.stringify(raw)}`) };
  }
  switch (event.event) {
    case "hello":
      return event.version === PROTOCOL_VERSION
        ? { ...state, engine: event.engine }
        : {
            ...state,
            engine: event.engine,
            error: `This window and its engine don't match (protocol ${event.version}, expected ${PROTOCOL_VERSION}). Reinstall TwinCursor.`,
          };
    case "connection":
      return { ...state, connection: event.state };
    case "participants":
      return { ...state, participants: event.participants };
    case "control_asked":
      return {
        ...state,
        asked: { request: event.request, identity: event.identity, name: event.name, platform: event.platform },
      };
    case "control":
      return {
        ...state,
        control: { state: event.state, by: event.by },
        // Granted, refused, withdrawn or over: there's nothing left to answer.
        asked: event.state === "asked" ? state.asked : null,
      };
    case "voice":
      return {
        ...state,
        voice: { sending: event.sending, muted: event.muted, mic: event.mic, speaker: event.speaker },
      };
    case "stats":
      return {
        ...state,
        stats: {
          width: event.width,
          height: event.height,
          fps: event.fps,
          kbps: event.kbps,
          rtt_ms: event.rtt_ms,
          codec: event.codec,
          shown_pct: event.shown_pct,
        },
      };
    case "error":
      return { ...state, error: event.message };
    case "ended":
      return { ...state, ended: event.reason };
  }
}

export function callReducer(state: CallState, action: CallAction): CallState {
  switch (action.type) {
    case "answered":
      return { ...state, asked: null };
    case "dismiss":
      return { ...state, endMessage: null, error: state.phase === "idle" ? null : state.error, notice: null };
    case "start_failed":
      return { ...state, phase: "idle", endMessage: action.message };
    case "output":
      break;
  }
  const output = action.output;
  switch (output.kind) {
    case "started":
      // A restart keeps the call's notice and errors; a new call starts clean.
      return state.phase === "in_call"
        ? { ...state, ...engineReset, codec: output.codec, args: output.args }
        : {
            ...initialCall,
            ...engineReset,
            phase: "in_call",
            role: output.role,
            codec: output.codec,
            args: output.args,
            logs: state.logs,
          };
    case "event":
      return onEvent(state, output.event);
    case "log":
      return { ...state, logs: addLog(state.logs, output.line) };
    case "restarting":
      return { ...state, ...engineReset, codec: output.codec, notice: output.message };
    case "exited":
      return {
        ...initialCall,
        logs: state.logs,
        endMessage: endMessage(state.ended, state.error, output.code),
      };
  }
}

/** The name as typed, marked as such: names are never proof of who someone is. */
export const unverified = (name: string): string => `${name} (name not verified)`;

const codecLabel = (codec: string): string =>
  codec === "h264" ? "H.264" : codec === "h265" ? "HEVC" : codec.toUpperCase();

/** One line of live numbers. */
export function statsLine(stats: Stats | null, role: CallRole | null): string {
  if (stats === null) {
    return role === "join" ? "No video yet." : "Not sending video yet.";
  }
  const parts = [
    `${stats.width}×${stats.height}`,
    `${stats.fps.toFixed(1)} fps`,
    `${Math.round(stats.kbps)} kbps`,
    stats.rtt_ms === null ? "round trip –" : `round trip ${Math.round(stats.rtt_ms)} ms`,
  ];
  if (stats.codec !== null) {
    parts.push(codecLabel(stats.codec));
  }
  if (role === "join" && stats.shown_pct !== null) {
    parts.push(stats.shown_pct === 100 ? "shown 1:1" : `shown at ${stats.shown_pct}%`);
  }
  return parts.join(" · ");
}

/** The numbers as a tester pastes them into a report. */
export function numbersReport(state: CallState, now: Date): string {
  const s = state.stats;
  const lines = [
    `TwinCursor call numbers, ${now.toISOString()}`,
    `role: ${state.role ?? "-"}, engine ${state.engine ?? "-"}, codec chosen: ${state.codec ?? "-"}`,
    `connection: ${state.connection ?? "-"}, others in the call: ${state.participants.length}`,
  ];
  if (s === null) {
    lines.push("video: none yet");
  } else {
    lines.push(
      `video: ${s.width}x${s.height}, ${s.fps.toFixed(1)} fps, ${Math.round(s.kbps)} kbps, ` +
        `round trip ${s.rtt_ms === null ? "-" : `${Math.round(s.rtt_ms)} ms`}, codec ${s.codec ?? "-"}` +
        (s.shown_pct === null ? "" : `, shown ${s.shown_pct}%`),
    );
  }
  if (state.notice !== null) {
    lines.push(`note: ${state.notice}`);
  }
  return lines.join("\n");
}
