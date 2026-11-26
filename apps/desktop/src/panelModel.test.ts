import { describe, expect, it } from "vitest";

import { initialCall } from "./call";
import { parseHomeView } from "./home";
import { panelModel, type PanelModel } from "./panelModel";
import { phraseText } from "./people";
import { findScenario, scenariosOf } from "./scenarios";
import { replay } from "./useSurface";

function model(state: string, local?: Parameters<typeof panelModel>[2]): PanelModel {
  const s = findScenario("panel", state);
  if (s === undefined) {
    throw new Error(`no panel scenario ${state}`);
  }
  return panelModel(s.home, replay(s.outputs), local);
}

function home(state: string): Extract<PanelModel, { kind: "home" }> {
  const m = model(state);
  if (m.kind !== "home") {
    throw new Error(`${state} is ${m.kind}`);
  }
  return m;
}

describe("panel scenarios", () => {
  it("has every state the dev page promises", () => {
    const names = scenariosOf("panel").map((s) => s.name);
    for (const name of ["off", "out", "signing", "home", "empty", "many", "ring", "needs", "starting", "hosting", "knocking", "joined", "error", "offline"]) {
      expect(names).toContain(name);
    }
  });

  it("are home views the Rust side could send", () => {
    for (const s of scenariosOf("panel")) {
      expect(parseHomeView(JSON.parse(JSON.stringify(s.home))), s.name).toEqual(s.home);
    }
  });
});

describe("the panel", () => {
  it("waits for the first home view", () => {
    expect(panelModel(null, initialCall).kind).toBe("loading");
  });

  it("says when the build has no server, and when you're signed out", () => {
    expect(model("off")).toEqual({ kind: "off" });
    expect(model("out")).toEqual({ kind: "signed_out", waiting: false });
    expect(model("signing")).toEqual({ kind: "signed_out", waiting: true });
    expect(model("out", { dismissed: new Set(), error: null, signingIn: true })).toEqual({ kind: "signed_out", waiting: true });
  });

  it("shows who is signed in", () => {
    expect(home("home").me).toEqual({ name: "Alex Morgan", handle: "alexmorgan", avatar: null });
  });

  it("offers Share and Paste link when idle, with a Call button for everyone", () => {
    const m = home("home");
    expect(m.main).toEqual({ kind: "share", busy: false, primary: true });
    expect(m.people.map((p) => p.name)).toEqual(["Sam Rivera", "Kim Lee", "Priya Natarajan"]);
    expect(m.people.every((p) => p.action.kind === "call" && p.action.enabled)).toBe(true);
    expect(m.rings).toEqual([]);
    expect(m.needsScreenRecording).toBe(false);
    expect([m.error, m.notice, m.serverProblem]).toEqual([null, null, null]);
  });

  it("lists nobody when there's nobody", () => {
    expect(home("empty").people).toEqual([]);
  });

  it("lists twelve, keeping long and non-Latin names whole for the view to truncate", () => {
    const m = home("many");
    expect(m.people).toHaveLength(12);
    expect(m.people.map((p) => p.name)).toContain("Bartholomew Montgomery-Fitzgerald Junior");
    expect(m.people.map((p) => p.name)).toContain("王小明");
    expect(m.people.find((p) => p.name === "Dev Patel")?.handle).toBeNull();
  });

  it("puts a ring on top and gives it the primary button", () => {
    const m = home("ring");
    expect(m.rings).toEqual([{ callId: "c7", name: "Sam Rivera", avatar: null }]);
    expect(m.main).toEqual({ kind: "share", busy: false, primary: false });
    expect(home("rings").rings).toHaveLength(2);
  });

  it("asks for Screen Recording above the main action", () => {
    expect(home("needs").needsScreenRecording).toBe(true);
  });

  it("shows a busy Share while starting, and Call can't be clicked", () => {
    const m = home("starting");
    expect(m.main).toEqual({ kind: "share", busy: true, primary: true });
    expect(m.people.every((p) => p.action.kind === "call" && !p.action.enabled)).toBe(true);
  });

  it("shows one status row while sharing, with the link, and rings people into the call", () => {
    const m = home("hosting");
    expect(m.main.kind).toBe("call");
    if (m.main.kind !== "call") {
      return;
    }
    expect(phraseText(m.main.line)).toBe("You're sharing your screen");
    expect(m.main.action).toEqual({ label: "Stop sharing", op: "end" });
    expect(m.main.link).toBe("twincursor://call/0123456789abcdef0123456789abcdef");
    expect(m.main.dot).toBe("good");
    expect(m.notice).toBe("The call link is on your clipboard.");
    const byName = Object.fromEntries(m.people.map((p) => [p.name, p.action]));
    expect(byName["Sam Rivera"]).toEqual({ kind: "status", label: "In the call" });
    expect(byName["Kim Lee"]).toEqual({ kind: "status", label: "Ringing…" });
    expect(byName["Priya Natarajan"]).toEqual({ kind: "call", enabled: true });
  });

  it("waits to be let in, with Cancel, and no Call buttons", () => {
    const m = home("knocking");
    expect(m.main.kind === "call" && phraseText(m.main.line)).toBe("Waiting for Alex to let you in…");
    expect(m.main.kind === "call" && m.main.action).toEqual({ label: "Cancel", op: "cancel_knock" });
    expect(m.main.kind === "call" && m.main.dot).toBe("pending");
    expect(m.people.every((p) => p.action.kind === "none")).toBe(true);
  });

  it("shows whose call you're in, with Leave", () => {
    const m = home("joined");
    expect(m.main.kind === "call" && phraseText(m.main.line)).toBe("In Alex's call");
    expect(m.main.kind === "call" && m.main.action).toEqual({ label: "Leave", op: "end" });
    expect(m.main.kind === "call" && m.main.link).toBeNull();
  });

  it("shows an error until it's closed, and a refusal of its own first", () => {
    expect(home("error").error).toBe("That isn't a TwinCursor call link.");
    const s = findScenario("panel", "error");
    if (s === undefined) {
      throw new Error("no error scenario");
    }
    const closed = panelModel(s.home, initialCall, { dismissed: new Set(["That isn't a TwinCursor call link."]), error: null, signingIn: false });
    expect(closed.kind === "home" && closed.error).toBeNull();
    const own = panelModel(s.home, initialCall, { dismissed: new Set(), error: "Sign in first.", signingIn: false });
    expect(own.kind === "home" && own.error).toBe("Sign in first.");
  });

  it("pins a server problem to the bottom", () => {
    expect(home("offline").serverProblem).toBe("Can't reach the server.");
  });

  it("loads only GitHub avatars", () => {
    const s = findScenario("panel", "home");
    if (s === undefined) {
      throw new Error("no home scenario");
    }
    const [first, second] = s.home.contacts;
    if (first === undefined || second === undefined) {
      throw new Error("too few contacts");
    }
    const contacts = [
      { ...first, avatar_url: "https://avatars.githubusercontent.com/u/1?v=4" },
      { ...second, avatar_url: "https://example.com/tracker.png" },
    ];
    const m = panelModel({ ...s.home, contacts }, initialCall);
    expect(m.kind === "home" && m.people.map((p) => p.avatar)).toEqual(["https://avatars.githubusercontent.com/u/1?v=4", null]);
  });
});
