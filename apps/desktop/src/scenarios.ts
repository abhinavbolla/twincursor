// Canned states of the panel and the call bar, as plain data: the home view the Rust side would
// send, and the engine output a call would have produced so far (replayed through the reducer,
// as a window opened mid-call does). The dev page shows them (`?view=panel&state=home`) and the
// view-model tests run over every one of them.

import type { AccountState } from "./accountState";
import type { HomeView, Person } from "./home";
import { PROTOCOL_VERSION, type CallRole, type Codec, type ConnectionState, type ControlState, type EngineOutput, type Participant, type Peer } from "./ipc";

export type Surface = "panel" | "bar";

export interface Scenario {
  surface: Surface;
  /** The `state` in the dev page's URL. */
  name: string;
  /** What it shows, in a few words. */
  title: string;
  home: HomeView;
  outputs: EngineOutput[];
  /** Bar: the menu section starts open. Panel: the account menu does. */
  menu?: boolean;
  /** Panel: opens on the Settings page. */
  settings?: boolean;
}

// ---------------------------------------------------------------- people

const person = (id: string, name: string, handle: string | null): Person => ({ id, name, handle, avatar_url: null });

export const ALEX = person("u1", "Alex Morgan", "alexmorgan");
export const SAM = person("u2", "Sam Rivera", "samrivera");
export const KIM = person("u3", "Kim Lee", "kimlee");
export const PRIYA = person("u4", "Priya Natarajan", "priya");
/** Forty characters: the longest name the surfaces are checked with. */
export const LONG = person("u5", "Bartholomew Montgomery-Fitzgerald Junior", "bartholomew-montgomery");

/** Someone without a GitHub handle. */
export const DEV = person("u13", "Dev Patel", null);

/** Twelve contacts, with a long name and names in non-Latin scripts. */
export const TWELVE: Person[] = [
  SAM,
  KIM,
  PRIYA,
  LONG,
  person("u6", "王小明", "wangxiaoming"),
  person("u7", "Иван Петров", "ivanpetrov"),
  person("u8", "山田 太郎", "yamada"),
  person("u9", "محمد الأحمد", "malahmad"),
  person("u10", "Zoë Ångström", "zoe"),
  person("u11", "Noah Williams", "noahw"),
  person("u12", "Lena Fischer", "lfischer"),
  DEV,
];

/** The engine identity of a guest with an account (`guest-<user id>`, as calls.rs makes it). */
export const guestIdentity = (p: Person): string => `guest-${p.id}`;
export const sharerIdentity = (p: Person): string => `sharer-${p.id}`;

export const asGuest = (p: Person, decoders: string[] = ["h264", "h265"]): Participant => ({
  identity: guestIdentity(p),
  name: p.name,
  role: "guest",
  decoders,
});
export const asSharer = (p: Person): Participant => ({ identity: sharerIdentity(p), name: p.name, role: "sharer", decoders: [] });
export const peer = (p: Participant): Peer => ({ identity: p.identity, name: p.name });

// ---------------------------------------------------------------- engine output

const ev = (event: Record<string, unknown>): EngineOutput => ({ kind: "event", event });

export const out = {
  started: (role: CallRole, codec: Codec = "h264"): EngineOutput => ({
    kind: "started",
    role,
    codec,
    pid: 4242,
    args: role === "share" ? ["share", "--ipc", `--codec=${codec}`, "--allow-control"] : ["view", "--ipc"],
  }),
  hello: (role: CallRole): EngineOutput =>
    ev({ event: "hello", version: PROTOCOL_VERSION, role: role === "share" ? "sharer" : "guest", engine: "0.1.0" }),
  connection: (state: ConnectionState): EngineOutput => ev({ event: "connection", state }),
  participants: (participants: Participant[]): EngineOutput => ev({ event: "participants", participants }),
  voice: (muted = false, sending = true): EngineOutput =>
    ev({
      event: "voice",
      sending,
      muted,
      mic: sending ? "MacBook Pro Microphone" : null,
      speaker: sending ? "MacBook Pro Speakers" : null,
    }),
  stats: (role: CallRole, codec: Codec = "h265"): EngineOutput =>
    ev({
      event: "stats",
      width: role === "share" ? 3840 : 1920,
      height: role === "share" ? 2160 : 1080,
      fps: 59.8,
      kbps: codec === "h265" ? 9184 : 2612,
      rtt_ms: 4.2,
      codec,
      shown_pct: role === "join" ? 100 : null,
    }),
  control: (state: ControlState, by: Peer | null, trusted: Peer[] = []): EngineOutput =>
    ev({ event: "control", state, by, trusted }),
  asked: (request: number, p: Participant): EngineOutput =>
    ev({ event: "control_asked", request, identity: p.identity, name: p.name, platform: "macos" }),
  error: (message: string): EngineOutput => ev({ event: "error", message }),
  restarting: (codec: Codec, message: string): EngineOutput => ({ kind: "restarting", codec, message }),
  exited: (code: number | null = 0): EngineOutput => ({ kind: "exited", code }),
};

/** An engine that has started and connected, with voice on. */
export const boot = (role: CallRole, codec: Codec = role === "share" ? "h265" : "h264"): EngineOutput[] => [
  out.started(role, codec),
  out.hello(role),
  out.connection("connecting"),
  out.connection("connected"),
  out.voice(),
];

// ---------------------------------------------------------------- home views

const signedIn = (me: Person): AccountState => ({ configured: true, signed_in: true, me, signing_in: false });

export const LINK = "twincursor://call/0123456789abcdef0123456789abcdef";

export function homeView(change: Partial<HomeView> = {}): HomeView {
  return {
    account: signedIn(ALEX),
    stage: { kind: "idle" },
    contacts: [SAM, KIM, PRIYA],
    rings: [],
    knocks: [],
    members: [],
    notice: null,
    error: null,
    needs: null,
    server_problem: null,
    ...change,
  };
}

const hosting = (change: Partial<HomeView> = {}): HomeView =>
  homeView({ stage: { kind: "hosting", call_id: "c1", link: LINK }, ...change });

/** Sam, as a guest of Alex's call. */
const guestHome = (stage: HomeView["stage"], change: Partial<HomeView> = {}): HomeView =>
  homeView({ account: signedIn(SAM), contacts: [ALEX, KIM, PRIYA], stage, ...change });

const admitted = (...people: Person[]): HomeView["members"] =>
  people.map((p) => ({ user_id: p.id, name: p.name, handle: p.handle, avatar_url: p.avatar_url, state: "admitted" }));

const knock = (p: Person, armed: boolean): HomeView["knocks"][number] => ({
  user_id: p.id,
  name: p.name,
  handle: p.handle,
  avatar_url: p.avatar_url,
  armed,
});

// ---------------------------------------------------------------- the scenarios

const SAM_IN = asGuest(SAM);
const KIM_IN = asGuest(KIM, ["h264", "vp8"]);
const ALEX_SHARING = asSharer(ALEX);
const SAM_WATCHING = { identity: guestIdentity(SAM), name: SAM.name };

/** Alex shares; Sam is in the call. */
const withSam = (...more: EngineOutput[]): EngineOutput[] => [
  ...boot("share"),
  out.participants([SAM_IN]),
  out.stats("share"),
  ...more,
];

/** Sam watches Alex's screen. */
const watching = (...more: EngineOutput[]): EngineOutput[] => [
  ...boot("join"),
  out.participants([ALEX_SHARING]),
  out.stats("join", "h264"),
  ...more,
];

const panel = (name: string, title: string, home: HomeView, outputs: EngineOutput[] = []): Scenario => ({
  surface: "panel",
  name,
  title,
  home,
  outputs,
});

const bar = (name: string, title: string, home: HomeView, outputs: EngineOutput[], menu = false): Scenario => ({
  surface: "bar",
  name,
  title,
  home,
  outputs,
  menu,
});

export const SCENARIOS: readonly Scenario[] = [
  panel("off", "This build names no server", homeView({ account: { configured: false, signed_in: false, me: null, signing_in: false } })),
  panel("out", "Signed out", homeView({ account: { configured: true, signed_in: false, me: null, signing_in: false }, contacts: [] })),
  {
    ...panel("out-menu", "Signed out, with the menu open", homeView({ account: { configured: true, signed_in: false, me: null, signing_in: false }, contacts: [] })),
    menu: true,
  },
  panel("signing", "Signing in, waiting for the browser", homeView({ account: { configured: true, signed_in: false, me: null, signing_in: true }, contacts: [] })),
  panel("home", "Signed in, three people", homeView()),
  panel("empty", "Signed in, nobody yet", homeView({ contacts: [] })),
  panel("many", "Twelve people, long and non-Latin names", homeView({ contacts: TWELVE })),
  panel("ring", "Sam is calling", homeView({ rings: [{ call_id: "c7", host: SAM }] })),
  panel("rings", "Two people calling at once", homeView({ rings: [{ call_id: "c7", host: SAM }, { call_id: "c8", host: LONG }] })),
  panel("needs", "Screen Recording is missing", homeView({ needs: "screen_recording" })),
  panel("starting", "Share was clicked", homeView({ stage: { kind: "starting" } })),
  panel(
    "hosting",
    "Sharing, Sam in the call, Kim being rung",
    hosting({
      members: [...admitted(SAM), { user_id: KIM.id, name: KIM.name, handle: KIM.handle, avatar_url: null, state: "ringing" }],
      notice: "The call link is on your clipboard.",
    }),
    withSam(),
  ),
  panel("knocking", "Waiting to be let in", guestHome({ kind: "knocking", host: ALEX })),
  panel("joined", "In Alex's call", guestHome({ kind: "joined", host: ALEX }), watching()),
  panel("error", "A paste that wasn't a link", homeView({ error: "That isn't a TwinCursor call link." })),
  { ...panel("settings", "The Settings page", homeView()), settings: true },
  panel("offline", "The server can't be reached", homeView({ server_problem: "Can't reach the server." })),

  bar("alone", "Sharing, nobody yet", hosting(), boot("share")),
  bar("knock", "Sam knocks, Let in not armed yet", hosting({ knocks: [knock(SAM, false)] }), boot("share")),
  bar("knock2", "Two knocks, both armed", hosting({ knocks: [knock(SAM, true), knock(LONG, true)] }), boot("share")),
  bar("knock-nohandle", "A knock from someone without a handle", hosting({ knocks: [knock(DEV, true)] }), boot("share")),
  bar("with", "Sam is in the call", hosting({ members: admitted(SAM) }), withSam()),
  bar("trusted", "Sam may take control", hosting({ members: admitted(SAM) }), withSam(out.control("none", null, [peer(SAM_IN)]))),
  bar(
    "controlling",
    "Sam is controlling",
    hosting({ members: admitted(SAM) }),
    withSam(out.control("controlling", peer(SAM_IN), [peer(SAM_IN)])),
  ),
  bar(
    "asked",
    "Sam asks for control; Allow arms after a second",
    hosting({ members: admitted(SAM) }),
    withSam(out.asked(1, SAM_IN), out.control("asked", peer(SAM_IN))),
  ),
  bar("needs", "Accessibility is missing", hosting({ members: admitted(SAM), needs: "accessibility" }), withSam()),
  bar(
    "menu",
    "The menu, with numbers",
    hosting({ members: admitted(SAM, KIM) }),
    [...boot("share"), out.participants([SAM_IN, KIM_IN]), out.stats("share"), out.control("none", null, [peer(SAM_IN)])],
    true,
  ),
  bar(
    "notice",
    "Switched to H.264 for Kim, and the link notice",
    hosting({ members: admitted(SAM, KIM), notice: "The call link is on your clipboard." }),
    [
      ...boot("share", "h265"),
      out.participants([SAM_IN, KIM_IN]),
      out.restarting("h264", "Kim Lee's computer can't decode HEVC; switched to H.264."),
      ...boot("share", "h264"),
      out.participants([SAM_IN, KIM_IN]),
      out.stats("share", "h264"),
    ],
  ),
  bar("g-knocking", "Guest: waiting to be let in", guestHome({ kind: "knocking", host: ALEX }), []),
  bar("g-watching", "Guest: watching Alex's screen", guestHome({ kind: "joined", host: ALEX }), watching()),
  bar(
    "g-asked",
    "Guest: asked for control",
    guestHome({ kind: "joined", host: ALEX }),
    watching(out.control("asked", SAM_WATCHING)),
  ),
  bar(
    "g-controlling",
    "Guest: controlling Alex's Mac",
    guestHome({ kind: "joined", host: ALEX }),
    watching(out.control("controlling", SAM_WATCHING)),
  ),
  bar("reconnecting", "The connection dropped", hosting({ members: admitted(SAM) }), withSam(out.connection("reconnecting"))),
  bar("muted", "Your mic is muted", hosting({ members: admitted(SAM) }), withSam(out.voice(true))),
  bar(
    "long-trusted",
    "A long first name, trusted, with four guests",
    hosting({ members: admitted(LONG, SAM, KIM, PRIYA) }),
    [
      ...boot("share"),
      out.participants([asGuest(LONG), SAM_IN, KIM_IN, asGuest(PRIYA)]),
      out.control("none", null, [peer(asGuest(LONG))]),
    ],
  ),
  bar(
    "long-controlling",
    "A long first name, controlling",
    hosting({ members: admitted(LONG) }),
    [...boot("share"), out.participants([asGuest(LONG)]), out.control("controlling", peer(asGuest(LONG)), [peer(asGuest(LONG))])],
  ),
  bar(
    "long-g-controlling",
    "Guest: controlling the Mac of a host with a long first name",
    guestHome({ kind: "joined", host: LONG }),
    [...boot("join"), out.participants([asSharer(LONG)]), out.control("controlling", SAM_WATCHING)],
  ),
];

export const scenariosOf = (surface: Surface): Scenario[] => SCENARIOS.filter((s) => s.surface === surface);

export const findScenario = (surface: Surface, name: string): Scenario | undefined =>
  SCENARIOS.find((s) => s.surface === surface && s.name === name);
