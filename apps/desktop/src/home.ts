// Accounts and calls as the window sees them (`HomeView` in src-tauri/src/calls.rs): who is
// signed in, where this app is with a call, who is knocking or ringing. The Rust side keeps it
// and sends all of it on every change (the `home` event, and `home_state`). Never a token, a
// pass or what is on the clipboard.

import { parseAccountState, type AccountState, type Me } from "./accountState";

export type Person = Me;

export type Stage =
  | { kind: "idle" }
  | { kind: "starting" }
  | { kind: "hosting"; call_id: string; link: string }
  | { kind: "knocking"; host: Person }
  | { kind: "joined"; host: Person }
  | { kind: "ending" };

export type MemberState =
  | "ringing"
  | "knocking"
  | "admitted"
  | "turned_away"
  | "declined"
  | "left"
  | "missed"
  | "unknown";

export interface RingView {
  call_id: string;
  host: Person;
}

export interface KnockView {
  user_id: string;
  name: string;
  handle: string | null;
  avatar_url: string | null;
  /** Let in can be clicked: the Rust side refuses it for a second after the knock is shown. */
  armed: boolean;
}

export interface MemberView {
  user_id: string;
  name: string;
  handle: string | null;
  avatar_url: string | null;
  state: MemberState;
}

export type NeededPermission = "screen_recording" | "accessibility";

export interface HomeView {
  account: AccountState;
  stage: Stage;
  contacts: Person[];
  rings: RingView[];
  knocks: KnockView[];
  members: MemberView[];
  notice: string | null;
  error: string | null;
  needs: NeededPermission | null;
  server_problem: string | null;
}

type Obj = Record<string, unknown>;
const isObj = (v: unknown): v is Obj => typeof v === "object" && v !== null && !Array.isArray(v);
const isStr = (v: unknown): v is string => typeof v === "string";
const isStrOrNull = (v: unknown): v is string | null => v === null || typeof v === "string";

const MEMBER_STATES: readonly MemberState[] = [
  "ringing",
  "knocking",
  "admitted",
  "turned_away",
  "declined",
  "left",
  "missed",
  "unknown",
];

function parsePerson(v: unknown): Person | null {
  if (!isObj(v) || !isStr(v.id) || !isStr(v.name) || !isStrOrNull(v.handle) || !isStrOrNull(v.avatar_url)) {
    return null;
  }
  return { id: v.id, name: v.name, handle: v.handle, avatar_url: v.avatar_url };
}

function parseStage(v: unknown): Stage | null {
  if (!isObj(v)) {
    return null;
  }
  switch (v.kind) {
    case "idle":
    case "starting":
    case "ending":
      return { kind: v.kind };
    case "hosting":
      return isStr(v.call_id) && isStr(v.link) ? { kind: "hosting", call_id: v.call_id, link: v.link } : null;
    case "knocking":
    case "joined": {
      const host = parsePerson(v.host);
      return host === null ? null : { kind: v.kind, host };
    }
    default:
      return null;
  }
}

/** Every element read by `parse`, or null if any can't be. */
function list<T>(v: unknown, parse: (x: unknown) => T | null): T[] | null {
  if (!Array.isArray(v)) {
    return null;
  }
  const out: T[] = [];
  for (const x of v) {
    const parsed = parse(x);
    if (parsed === null) {
      return null;
    }
    out.push(parsed);
  }
  return out;
}

function parseRing(v: unknown): RingView | null {
  if (!isObj(v) || !isStr(v.call_id)) {
    return null;
  }
  const host = parsePerson(v.host);
  return host === null ? null : { call_id: v.call_id, host };
}

function parseKnock(v: unknown): KnockView | null {
  if (
    !isObj(v) ||
    !isStr(v.user_id) ||
    !isStr(v.name) ||
    !isStrOrNull(v.handle) ||
    !isStrOrNull(v.avatar_url) ||
    typeof v.armed !== "boolean"
  ) {
    return null;
  }
  return { user_id: v.user_id, name: v.name, handle: v.handle, avatar_url: v.avatar_url, armed: v.armed };
}

function parseMember(v: unknown): MemberView | null {
  if (
    !isObj(v) ||
    !isStr(v.user_id) ||
    !isStr(v.name) ||
    !isStrOrNull(v.handle) ||
    !isStrOrNull(v.avatar_url) ||
    !MEMBER_STATES.includes(v.state as MemberState)
  ) {
    return null;
  }
  return {
    user_id: v.user_id,
    name: v.name,
    handle: v.handle,
    avatar_url: v.avatar_url,
    state: v.state as MemberState,
  };
}

/** Reads the Rust side's home view, or null if it isn't one. */
export function parseHomeView(value: unknown): HomeView | null {
  if (!isObj(value)) {
    return null;
  }
  const v = value;
  const account = parseAccountState(v.account);
  const stage = parseStage(v.stage);
  const contacts = list(v.contacts, parsePerson);
  const rings = list(v.rings, parseRing);
  const knocks = list(v.knocks, parseKnock);
  const members = list(v.members, parseMember);
  const needs = v.needs === null || v.needs === "screen_recording" || v.needs === "accessibility" ? v.needs : undefined;
  if (
    account === null ||
    stage === null ||
    contacts === null ||
    rings === null ||
    knocks === null ||
    members === null ||
    needs === undefined ||
    !isStrOrNull(v.notice) ||
    !isStrOrNull(v.error) ||
    !isStrOrNull(v.server_problem)
  ) {
    return null;
  }
  return {
    account,
    stage,
    contacts,
    rings,
    knocks,
    members,
    notice: v.notice,
    error: v.error,
    needs,
    server_problem: v.server_problem,
  };
}
