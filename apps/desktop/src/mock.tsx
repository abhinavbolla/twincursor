// A pretend engine for trying the window in a browser (`npm run dev`), with nobody to call.
// Development only: bridge.ts imports this file behind `import.meta.env.DEV`, so the bundled
// app doesn't contain it.
//
//   http://localhost:5173/?mock=share   starts a call as the sharer
//   http://localhost:5173/?mock=join    starts a call as a guest
//   add &quality=4k to share with HEVC, &voice=off for a call without voice
//
// The panel in the corner makes things happen: a control request, a guest without HEVC, an
// error, a dropped connection, the engine crashing, and permissions changing.

import type { Bridge, EngineConfig, LoadedBridge, SettingsChange, SettingsView } from "./bridge";
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
  private grant: number | null = null;
  /** A call to start once the window listens (`?mock=`). */
  pending: Plan | null = null;
  permissions: PermissionStatus = {
    screen_recording: "not_granted",
    accessibility: "not_granted",
    microphone: "not_asked",
  };
  settings: { url: string | null; api_key: string | null; secret: string | null } = {
    url: null,
    api_key: null,
    secret: null,
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
        this.control("none", null);
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

function MockPanel({ engine }: { engine: MockEngine }) {
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

export function createMock(params: URLSearchParams): LoadedBridge {
  const engine = new MockEngine();
  const view = (): SettingsView => ({
    url: engine.settings.url,
    api_key: engine.settings.api_key,
    has_secret: engine.settings.secret !== null,
  });
  const config: EngineConfig = {
    url: "ws://localhost:7880",
    api_key: "devkey",
    has_secret: false,
    has_pass: false,
    source: "dev",
  };
  const bridge: Bridge = {
    engineInfo: () =>
      Promise.resolve({ path: "(mock engine)", error: null, in_bundle: false, app_pid: 1 }),
    startCall: (plan) => {
      if (engine.inCall) {
        return Promise.reject(new Error("A call is already running."));
      }
      window.setTimeout(() => engine.start(plan), 0);
      return Promise.resolve();
    },
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
    readSettings: () => Promise.resolve(view()),
    saveSettings: (change: SettingsChange) => {
      const filled = (v: string | null) => (v === null || v.trim() === "" ? null : v.trim());
      engine.settings.url = filled(change.url);
      engine.settings.api_key = filled(change.api_key);
      if (change.api_secret.do === "clear") {
        engine.settings.secret = null;
      } else if (change.api_secret.do === "set") {
        engine.settings.secret = filled(change.api_secret.value);
      }
      return Promise.resolve(view());
    },
    engineConfig: () => Promise.resolve(config),
    onOutput: (f) => engine.on(f),
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
  return { bridge, DevPanel: () => <MockPanel engine={engine} /> };
}
