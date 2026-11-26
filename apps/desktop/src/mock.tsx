// A pretend engine for trying the window in a browser (`npm run dev`), with nobody to call.
// Development only: bridge.ts imports this file behind `import.meta.env.DEV`, so the bundled
// app doesn't contain it.
//
//   http://localhost:5173/?mock=share   starts a call as the sharer
//   http://localhost:5173/?mock=join    starts a call as a guest
//   add &quality=4k to share with HEVC, &voice=off for a call without voice
//   add &account=off for a build without an account backend, &account=in to start signed in,
//   &account=uninvited to start signed in on a server that doesn't let you start calls
//
// The panel in the corner makes things happen: a control request, a guest without HEVC, an
// error, a dropped connection, the engine crashing, and permissions changing; and for calls
// with accounts, someone knocking or ringing, the host letting you in or turning you away, the
// call ending on the server, the server going away, and a permission that is missing.

import type { AccountState } from "./accountState";
import type { Bridge, LoadedBridge } from "./bridge";
import type { HomeView, Person } from "./home";
import type { EngineCommand, EngineOutput, Participant, Plan } from "./ipc";
import type { Access, PermissionName, PermissionStatus } from "./permissions";

type Listener = (output: EngineOutput) => void;

const SAM: Participant = {
  identity: "guest-5a3f09c1d2e4b6a7",
  name: "Sam",
  role: "guest",
  decoders: [],
};
const ALEX: Participant = {
  identity: "sharer-0c9d8e7f6a5b4c3d",
  name: "Alex",
  role: "sharer",
  decoders: [],
};
const ALL_DECODERS = ["h264", "h265", "vp8", "vp9", "av1"];
const SIZES: Record<Plan["quality"], [number, number]> = {
  "1080p": [1920, 1080],
  "1440p": [2560, 1440],
  "4k": [3840, 2160],
};

class MockEngine {
  private listeners = new Set<Listener>();
  private timers: number[] = [];
  private ticker: number | null = null;
  private plan: Plan | null = null;
  private codec: "h264" | "h265" = "h264";
  private request = 0;
  private muted = false;
  private people: Participant[] = [];
  private controlling = false;
  /** Sharer: guests trusted with control for this call (`trust_control`), until a revoke. */
  private trusted: Participant[] = [];
  /** Sharer: guests to trust when they join (`trust_on_arrival`), until a revoke. */
  private expected: string[] = [];
  private grant: number | null = null;
  /** A call to start once the window listens (`?mock=`). */
  pending: Plan | null = null;
  permissions: PermissionStatus = {
    screen_recording: "not_granted",
    accessibility: "not_granted",
    microphone: "not_asked",
  };

  on(f: Listener): () => void {
    this.listeners.add(f);
    const pending = this.pending;
    if (pending !== null) {
      this.pending = null;
      window.setTimeout(() => this.start(pending), 300);
    }
    return () => this.listeners.delete(f);
  }

  private out(output: EngineOutput): void {
    for (const f of this.listeners) {
      f(output);
    }
  }

  private event(event: Record<string, unknown>): void {
    this.out({ kind: "event", event });
  }

  private log(line: string): void {
    this.out({ kind: "log", line: `${new Date().toISOString()}  INFO mock: ${line}` });
  }

  private later(ms: number, f: () => void): void {
    this.timers.push(window.setTimeout(f, ms));
  }

  private clear(): void {
    this.timers.forEach((t) => window.clearTimeout(t));
    this.timers = [];
    if (this.ticker !== null) {
      window.clearInterval(this.ticker);
      this.ticker = null;
    }
  }

  get inCall(): boolean {
    return this.plan !== null;
  }

  get role(): Plan["role"] | null {
    return this.plan?.role ?? null;
  }

  private participants(): void {
    const arrived = this.people.filter((p) => p.role === "guest" && this.expected.includes(p.identity));
    if (arrived.length > 0) {
      this.expected = this.expected.filter((id) => !arrived.some((p) => p.identity === id));
      if (!this.controlling) {
        this.trusted = [...this.trusted.filter((t) => !arrived.some((p) => p.identity === t.identity)), ...arrived];
      }
    }
    this.event({ event: "participants", participants: this.people });
  }

  private boot(plan: Plan): void {
    const args = [
      plan.role === "share" ? "share" : "view",
      "--ipc",
      `--room=${plan.room}`,
      `--name=${plan.name}`,
      ...(plan.voice ? [] : ["--audio=off"]),
      ...(plan.role === "share" ? [`--codec=${this.codec}`, "--allow-control"] : []),
    ];
    this.out({ kind: "started", role: plan.role, codec: this.codec, pid: 4242, args });
    this.event({
      event: "hello",
      version: 1,
      role: plan.role === "share" ? "sharer" : "guest",
      engine: "0.1.0-mock",
    });
    this.event({ event: "connection", state: "connecting" });
    this.log(`connecting to room ${plan.room}`);
    this.later(500, () => {
      this.event({ event: "connection", state: "connected" });
      this.event({
        event: "voice",
        sending: plan.voice,
        muted: this.muted,
        mic: plan.voice ? "MacBook Pro Microphone" : null,
        speaker: plan.voice ? "MacBook Pro Speakers" : null,
      });
    });
    if (plan.role === "share") {
      this.later(1500, () => {
        this.people = [{ ...SAM }];
        this.participants();
      });
      this.later(2300, () => {
        this.people = [{ ...SAM, decoders: ALL_DECODERS }];
        this.participants();
      });
    } else {
      this.later(900, () => {
        this.people = [{ ...ALEX }];
        this.participants();
      });
    }
    this.ticker = window.setInterval(() => this.stats(), 1000);
  }

  private stats(): void {
    if (this.plan === null) {
      return;
    }
    const [width, height] = SIZES[this.plan.quality];
    const jitter = Math.random();
    this.event({
      event: "stats",
      width: this.plan.role === "join" ? 1920 : width,
      height: this.plan.role === "join" ? 1080 : height,
      fps: 58 + jitter * 2,
      kbps: (this.codec === "h265" ? 9000 : 2400) + jitter * 400,
      rtt_ms: 3 + jitter * 4,
      codec: this.plan.role === "join" ? "h264" : this.codec,
      shown_pct: this.plan.role === "join" ? 100 : null,
    });
  }

  start(plan: Plan): void {
    this.clear();
    this.plan = plan;
    this.codec = plan.role === "share" && plan.quality === "4k" ? "h265" : "h264";
    this.muted = false;
    this.controlling = false;
    this.people = [];
    this.boot(plan);
  }

  private end(reason: "left" | "disconnected" | "failed" | null, code: number | null): void {
    this.clear();
    if (reason !== null) {
      this.event({ event: "connection", state: "disconnected" });
      this.event({ event: "ended", reason });
    }
    this.plan = null;
    this.later(150, () => this.out({ kind: "exited", code }));
  }

  private control(state: "none" | "asked" | "controlling", who: Participant | null): void {
    this.controlling = state === "controlling";
    this.event({
      event: "control",
      state,
      by: who === null ? null : { identity: who.identity, name: who.name },
      trusted: this.trusted.map((p) => ({ identity: p.identity, name: p.name })),
    });
  }

  command(command: EngineCommand): void {
    this.log(`command ${JSON.stringify(command)}`);
    const sharer = this.plan?.role === "share";
    const me: Participant = { ...SAM, name: this.plan?.name ?? "me", identity: "guest-me" };
    switch (command.cmd) {
      case "mute":
      case "unmute":
        this.muted = command.cmd === "mute";
        this.event({
          event: "voice",
          sending: this.plan?.voice ?? false,
          muted: this.muted,
          mic: this.plan?.voice ? "MacBook Pro Microphone" : null,
          speaker: this.plan?.voice ? "MacBook Pro Speakers" : null,
        });
        return;
      case "leave":
        this.end("left", 0);
        return;
      case "allow_control":
        if (command.request !== this.request) {
          this.event({ event: "error", message: "That control request is no longer open." });
          return;
        }
        this.control("controlling", SAM);
        return;
      case "deny_control":
        this.control("none", null);
        return;
      case "revoke_control":
        this.trusted = [];
        this.expected = [];
        this.control("none", null);
        return;
      case "trust_control": {
        const guest = this.people.find((p) => p.identity === command.identity && p.role === "guest");
        if (!sharer || guest === undefined) {
          this.event({
            event: "error",
            message: sharer ? "That person isn't in the call." : "Only the sharer can answer requests for control.",
          });
          return;
        }
        if (this.controlling && !this.trusted.some((p) => p.identity === guest.identity)) {
          this.event({ event: "error", message: "Someone else is in control. Take control back first." });
          return;
        }
        this.trusted = [...this.trusted.filter((p) => p.identity !== guest.identity), guest];
        this.control(this.controlling ? "controlling" : "none", this.controlling ? guest : null);
        return;
      }
      case "trust_on_arrival":
        if (!sharer || !command.identity.startsWith("guest-")) {
          this.event({ event: "error", message: sharer ? "Only a guest can be given control." : "Only the sharer can answer requests for control." });
          return;
        }
        if (this.people.some((p) => p.identity === command.identity)) {
          this.command({ cmd: "trust_control", identity: command.identity });
        } else {
          this.expected = [...this.expected.filter((id) => id !== command.identity), command.identity];
        }
        return;
      case "request_control":
        if (sharer) {
          this.event({ event: "error", message: "Only a guest can ask for control." });
          return;
        }
        this.control("asked", me);
        this.grant = window.setTimeout(() => this.control("controlling", me), 1500);
        return;
      case "release_control":
        if (this.grant !== null) {
          window.clearTimeout(this.grant);
          this.grant = null;
        }
        this.control("none", null);
        return;
      case "set_mic":
      case "set_speaker":
        return;
    }
  }

  // What the dev panel can make happen.

  guestAsks(): void {
    if (this.plan?.role !== "share" || this.controlling) {
      return;
    }
    if (this.trusted.some((p) => p.identity === SAM.identity)) {
      // Trusted: granted with no prompt, as the engine does.
      this.control("controlling", SAM);
      return;
    }
    this.request += 1;
    this.event({
      event: "control_asked",
      request: this.request,
      identity: SAM.identity,
      name: SAM.name,
      platform: "macos",
    });
    this.control("asked", SAM);
  }

  guestWithdraws(): void {
    this.control("none", null);
  }

  guestWithoutHevc(): void {
    if (this.plan?.role !== "share") {
      return;
    }
    const ana: Participant = {
      identity: "guest-77aa88bb99cc00dd",
      name: "Ana",
      role: "guest",
      decoders: ["h264", "vp8", "vp9", "av1"],
    };
    this.people = [...this.people, ana];
    this.participants();
    if (this.codec === "h265") {
      // What the Rust side does: a new engine, on H.264, for the rest of the call.
      this.out({
        kind: "restarting",
        codec: "h264",
        message: "Ana's computer can't decode HEVC; switched to H.264.",
      });
      this.clear();
      this.codec = "h264";
      const plan = this.plan;
      this.boot(plan);
    }
  }

  guestLeaves(): void {
    this.people = this.people.filter((p) => p.role !== "guest");
    this.trusted = [];
    if (this.controlling) {
      this.control("none", null);
    }
    this.participants();
  }

  error(): void {
    this.event({ event: "error", message: "The microphone stopped working; using the system default." });
  }

  dropConnection(): void {
    this.event({ event: "connection", state: "reconnecting" });
    this.later(2500, () => this.event({ event: "connection", state: "connected" }));
  }

  serverEnds(): void {
    this.end("disconnected", 0);
  }

  crash(): void {
    this.log("thread 'main' panicked at a mock place");
    this.end(null, 101);
  }

  failToStart(): void {
    this.event({ event: "error", message: "Sharing failed: no route to the server." });
    this.end("failed", 1);
  }
}


const ARMING_MS = 1000;
const MOCK_CODE = "0123456789abcdef0123456789abcdef";
const LINK = /^\s*(?:twincursor:\/\/call\/)?[0-9a-f]{32}\s*$/;
const SAM_PERSON: Person = { id: "u2", name: "Sam Example", handle: "sam", avatar_url: null };
const NOT_INVITED = "This server is invite-only. Ask whoever runs it to add your GitHub handle.";

/** Pretend account flows, as the Rust side runs them (src-tauri/src/calls.rs). */
class MockCalls {
  private listeners = new Set<(home: HomeView) => void>();
  private shown = new Map<string, number>();
  private missingPermission: HomeView["needs"] = null;
  /** What "Paste link" finds on the pretend clipboard. */
  clipboard: string = `twincursor://call/${MOCK_CODE}`;
  home: HomeView;

  constructor(
    private engine: MockEngine,
    private account: () => AccountState,
  ) {
    this.home = {
      account: account(),
      stage: { kind: "idle" },
      contacts: [SAM_PERSON],
      rings: [],
      knocks: [],
      members: [],
      notice: null,
      error: null,
      needs: null,
      server_problem: null,
    };
    window.setInterval(() => this.tick(), 500);
  }

  on(f: (home: HomeView) => void): () => void {
    this.listeners.add(f);
    return () => this.listeners.delete(f);
  }

  private set(change: Partial<HomeView>): HomeView {
    this.home = { ...this.home, ...change, account: this.account() };
    for (const f of this.listeners) {
      f(this.home);
    }
    return this.home;
  }

  /** Knocks arm a second after they were first shown. */
  private tick(): void {
    const now = Date.now();
    const knocks = this.home.knocks.map((k) => ({ ...k, armed: now - (this.shown.get(k.user_id) ?? now) >= ARMING_MS }));
    if (knocks.some((k, i) => k.armed !== this.home.knocks[i]?.armed) || this.home.account.signed_in !== this.account().signed_in) {
      this.set({ knocks });
    }
  }

  private check(): string | null {
    if (!this.account().signed_in) {
      return "Sign in first.";
    }
    if (this.home.stage.kind !== "idle" || this.engine.inCall) {
      return "You're already in a call. Leave it first.";
    }
    return null;
  }

  private startEngine(role: "share" | "join"): void {
    this.engine.start({ role, room: "", name: role === "share" ? "Alex" : "Sam", quality: "4k", voice: false });
  }

  share(): Promise<HomeView> {
    const problem = this.check();
    if (problem !== null) {
      return Promise.reject(problem);
    }
    if (!this.account().can_host) {
      return Promise.reject(NOT_INVITED);
    }
    if (this.missingPermission === "screen_recording") {
      this.set({ needs: "screen_recording" });
      return Promise.reject(
        "TwinCursor needs Screen Recording to share your screen. Turn it on in System Settings, then try again.",
      );
    }
    this.startEngine("share");
    return Promise.resolve(
      this.set({
        stage: { kind: "hosting", call_id: "c1", link: `twincursor://call/${MOCK_CODE}` },
        notice: "The call link is on your clipboard.",
        error: null,
        needs: null,
      }),
    );
  }

  joinLink(text: string): Promise<HomeView> {
    if (!LINK.test(text)) {
      return Promise.reject(text.trim() === "" ? "There's no call link to join. Copy the link first." : "That isn't a TwinCursor call link.");
    }
    const problem = this.check();
    if (problem !== null) {
      return Promise.reject(problem);
    }
    return Promise.resolve(this.set({ stage: { kind: "knocking", host: SAM_PERSON }, notice: null, error: null }));
  }

  joinFromClipboard(): Promise<HomeView> {
    return this.joinLink(this.clipboard);
  }

  cancelKnock(): Promise<HomeView> {
    if (this.home.stage.kind !== "knocking") {
      return Promise.reject("You aren't waiting to join a call.");
    }
    return Promise.resolve(this.set({ stage: { kind: "idle" } }));
  }

  answerKnock(userId: string, letIn: boolean, control: boolean): Promise<HomeView> {
    const knock = this.home.knocks.find((k) => k.user_id === userId);
    if (knock === undefined) {
      return Promise.reject("That person isn't waiting to join any more.");
    }
    if (letIn && Date.now() - (this.shown.get(userId) ?? Date.now()) < ARMING_MS) {
      return Promise.reject("Too soon. Try again.");
    }
    this.shown.delete(userId);
    const withoutControl = control && this.missingPermission === "accessibility";
    return Promise.resolve(
      this.set({
        knocks: this.home.knocks.filter((k) => k.user_id !== userId),
        members: [
          ...this.home.members,
          { user_id: knock.user_id, name: knock.name, handle: knock.handle, avatar_url: knock.avatar_url, state: letIn ? "admitted" : "turned_away" },
        ],
        notice: withoutControl ? `${knock.name} was let in without control: TwinCursor needs Accessibility to give control.` : null,
        needs: withoutControl ? "accessibility" : this.home.needs,
      }),
    );
  }

  giveControl(identity: string): Promise<HomeView> {
    if (this.home.stage.kind !== "hosting") {
      return Promise.reject("You aren't sharing a call.");
    }
    if (this.missingPermission === "accessibility") {
      this.set({ needs: "accessibility" });
      return Promise.reject("TwinCursor needs Accessibility to give control. Turn it on in System Settings, then try again.");
    }
    this.engine.command({ cmd: "trust_control", identity });
    return Promise.resolve(this.home);
  }

  async ring(userId: string): Promise<HomeView> {
    if (!this.account().can_host) {
      return Promise.reject(NOT_INVITED);
    }
    if (this.home.stage.kind !== "hosting") {
      await this.share();
    }
    const contact = this.home.contacts.find((c) => c.id === userId);
    if (contact === undefined) {
      return Promise.reject("You can only ring someone who has you as a contact too.");
    }
    const others = this.home.members.filter((m) => m.user_id !== userId);
    return this.set({
      members: [...others, { user_id: contact.id, name: contact.name, handle: contact.handle, avatar_url: contact.avatar_url, state: "ringing" }],
    });
  }

  async answerRing(callId: string, accept: boolean): Promise<HomeView> {
    const ring = this.home.rings.find((r) => r.call_id === callId);
    if (ring === undefined) {
      return Promise.reject("That call isn't ringing you any more.");
    }
    const rings = this.home.rings.filter((r) => r.call_id !== callId);
    if (!accept) {
      return this.set({ rings });
    }
    if (this.home.stage.kind !== "idle") {
      await this.end();
    }
    this.startEngine("join");
    return this.set({ rings, stage: { kind: "joined", host: ring.host } });
  }

  /** The account changed outside a flow (asked to host, invited): send the view again. */
  refresh(): HomeView {
    return this.set({});
  }

  end(): Promise<HomeView> {
    if (this.engine.inCall) {
      this.engine.command({ cmd: "leave" });
    }
    return Promise.resolve(this.set({ stage: { kind: "idle" }, knocks: [], members: [] }));
  }

  // What the dev panel can make happen.

  knock(): void {
    if (this.home.stage.kind !== "hosting" || this.home.knocks.some((k) => k.user_id === SAM_PERSON.id)) {
      return;
    }
    this.shown.set(SAM_PERSON.id, Date.now());
    this.set({
      knocks: [...this.home.knocks, { user_id: SAM_PERSON.id, name: SAM_PERSON.name, handle: SAM_PERSON.handle, avatar_url: null, armed: false }],
    });
  }

  ringMe(): void {
    if (!this.home.rings.some((r) => r.call_id === "c2")) {
      this.set({ rings: [...this.home.rings, { call_id: "c2", host: SAM_PERSON }] });
    }
  }

  admitted(): void {
    if (this.home.stage.kind === "knocking") {
      const host = this.home.stage.host;
      this.startEngine("join");
      this.set({ stage: { kind: "joined", host } });
    }
  }

  turnedAway(): void {
    if (this.home.stage.kind === "knocking") {
      this.set({ stage: { kind: "idle" }, notice: `${this.home.stage.host.name} didn't let you in.` });
    }
  }

  serverEnds(): void {
    if (this.home.stage.kind === "joined" || this.home.stage.kind === "hosting") {
      void this.end().then(() => this.set({ notice: "The call ended." }));
    }
  }

  offline(): void {
    this.set({ server_problem: this.home.server_problem === null ? "Can't reach the server." : null });
  }

  missing(which: "screen_recording" | "accessibility"): void {
    this.missingPermission = this.missingPermission === which ? null : which;
  }
}

function MockPanel({ engine, calls, account }: { engine: MockEngine; calls: MockCalls; account: MockAccount }) {
  const buttons: [string, () => void][] = [
    ["A guest asks for control", () => engine.guestAsks()],
    ["The guest withdraws", () => engine.guestWithdraws()],
    ["A guest without HEVC joins", () => engine.guestWithoutHevc()],
    ["The guests leave", () => engine.guestLeaves()],
    ["An error", () => engine.error()],
    ["The connection drops", () => engine.dropConnection()],
    ["The server ends the call", () => engine.serverEnds()],
    ["The engine fails", () => engine.failToStart()],
    ["The engine crashes", () => engine.crash()],
    ["Sam knocks on your call", () => calls.knock()],
    ["Sam rings you", () => calls.ringMe()],
    ["The host lets you in", () => calls.admitted()],
    ["The host turns you away", () => calls.turnedAway()],
    ["The call ends on the server", () => calls.serverEnds()],
    ["The server can't be reached", () => calls.offline()],
    ["Screen Recording is missing", () => calls.missing("screen_recording")],
    ["Accessibility is missing", () => calls.missing("accessibility")],
    [
      "You're invited, or not (invite-only server)",
      () => {
        account.toggleInvited();
        calls.refresh();
      },
    ],
  ];
  const set = (access: Access) => {
    engine.permissions = { screen_recording: access, accessibility: access, microphone: access };
  };
  return (
    <details className="mock-panel" aria-label="Mock engine">
      <summary>Mock engine (development only)</summary>
      {buttons.map(([label, run]) => (
        <button key={label} type="button" className="small secondary" onClick={run}>
          {label}
        </button>
      ))}
      <span className="muted">Permissions:</span>
      <button type="button" className="small secondary" onClick={() => set("granted")}>
        Allow all
      </button>
      <button type="button" className="small secondary" onClick={() => set("denied")}>
        Turn all off
      </button>
    </details>
  );
}

/** A pretend account: sign-in "finishes in the browser" after two seconds. */
class MockAccount {
  private state: AccountState;
  private pending: { timer: number; reject: (e: string) => void } | null = null;

  constructor(mode: string | null) {
    const signedIn = mode === "in" || mode === "uninvited";
    this.state = {
      configured: mode !== "off",
      signed_in: signedIn,
      me: signedIn ? MockAccount.ME : null,
      signing_in: false,
      can_host: mode !== "uninvited",
      asked_to_host: false,
    };
  }

  static readonly ME = { id: "u1", name: "Alex Example", handle: "alex", avatar_url: null };

  get(): Promise<AccountState> {
    return Promise.resolve(this.snapshot());
  }

  snapshot(): AccountState {
    return { ...this.state };
  }

  signIn(): Promise<AccountState> {
    if (!this.state.configured) {
      return Promise.reject("This build isn't set up for accounts.");
    }
    if (this.pending !== null) {
      return Promise.reject("Sign-in is already under way.");
    }
    this.state = { ...this.state, signing_in: true };
    return new Promise((resolve, reject) => {
      const timer = window.setTimeout(() => {
        this.pending = null;
        this.state = { ...this.state, configured: true, signed_in: true, me: MockAccount.ME, signing_in: false };
        resolve({ ...this.state });
      }, 2000);
      this.pending = { timer, reject };
    });
  }

  cancel(): Promise<void> {
    if (this.pending !== null) {
      window.clearTimeout(this.pending.timer);
      this.pending.reject("Sign-in was cancelled.");
      this.pending = null;
      this.state = { ...this.state, signing_in: false };
    }
    return Promise.resolve();
  }

  signOut(): Promise<AccountState> {
    this.state = { ...this.state, signed_in: false, me: null };
    return this.get();
  }

  askToHost(): void {
    this.state = { ...this.state, asked_to_host: true };
  }

  /** The owner adds or removes this person from the invite list. */
  toggleInvited(): void {
    this.state = { ...this.state, can_host: !this.state.can_host };
  }
}

export function createMock(params: URLSearchParams): LoadedBridge {
  const engine = new MockEngine();
  const account = new MockAccount(params.get("account"));
  const calls = new MockCalls(engine, () => account.snapshot());
  const bridge: Bridge = {
    engineInfo: () =>
      Promise.resolve({ path: "(mock engine)", error: null, in_bundle: false, app_pid: 1 }),
    send: (command) => {
      if (!engine.inCall) {
        return Promise.reject(new Error("No call is running."));
      }
      engine.command(command);
      return Promise.resolve();
    },
    leave: () => {
      if (engine.inCall) {
        engine.command({ cmd: "leave" });
      }
      return Promise.resolve();
    },
    permissions: (request) => {
      if (request) {
        const grant = (a: Access): Access => (a === "denied" ? a : "granted");
        const p = engine.permissions;
        engine.permissions = {
          screen_recording: grant(p.screen_recording),
          accessibility: grant(p.accessibility),
          microphone: grant(p.microphone),
        };
      }
      return Promise.resolve({ ...engine.permissions, pid: 1, parent_pid: 1, executable: "mock" });
    },
    openPane: (name: PermissionName) => {
      console.info(`(mock) would open System Settings for ${name}`);
      return Promise.resolve();
    },
    accountState: () => account.get(),
    signIn: () => account.signIn(),
    cancelSignIn: () => account.cancel(),
    signOut: () => account.signOut(),
    onOutput: (f) => engine.on(f),
    homeState: () => Promise.resolve(calls.home),
    callSnapshot: () => Promise.resolve([]),
    share: () => calls.share(),
    joinFromClipboard: () => calls.joinFromClipboard(),
    joinLink: (text) => calls.joinLink(text),
    cancelKnock: () => calls.cancelKnock(),
    answerKnock: (userId, letIn, control) => calls.answerKnock(userId, letIn, control),
    giveControl: (identity) => calls.giveControl(identity),
    ring: (userId) => calls.ring(userId),
    answerRing: (callId, accept) => calls.answerRing(callId, accept),
    askToHost: () => {
      if (!account.snapshot().signed_in) {
        return Promise.reject("Sign in first.");
      }
      account.askToHost();
      return Promise.resolve(calls.refresh());
    },
    end: () => calls.end(),
    onHome: (f) => calls.on(f),
    quit: () => {
      console.info("(mock) would quit TwinCursor");
      return Promise.resolve();
    },
    openGuide: () => {
      console.info("(mock) would open the self-hosting guide");
      return Promise.resolve();
    },
  };
  const role = params.get("mock");
  if (role === "share" || role === "join") {
    engine.pending = {
      role,
      room: "7kq2-m9xd-p4tz-hc3w",
      name: role === "share" ? "Alex" : "Sam",
      quality: params.get("quality") === "4k" ? "4k" : "1080p",
      voice: params.get("voice") !== "off",
    };
  }
  return { bridge, DevPanel: () => <MockPanel engine={engine} calls={calls} account={account} /> };
}
