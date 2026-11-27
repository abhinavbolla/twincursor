import { describe, expect, it } from "vitest";

import { initialCall } from "./call";
import { parseHomeView } from "./home";
import { NO_LOCAL, panelModel, type PanelModel } from "./panelModel";
import { RING_OUTCOME_MS, showingOutcomes, trackOutcomes } from "./ringOutcomes";
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
    for (const name of ["off", "out", "signing", "home", "empty", "many", "ring", "needs", "starting", "hosting", "rang-missed", "rang-declined", "knocking", "joined", "error", "offline", "uninvited", "uninvited-asked"]) {
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
    expect(model("out", { ...NO_LOCAL, signingIn: true })).toEqual({ kind: "signed_out", waiting: true });
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

  it("says what happened to a ring for 10 s after it is first seen, then offers Call again", () => {
    for (const [state, label] of [
      ["rang-missed", "No answer"],
      ["rang-declined", "Declined"],
    ] as const) {
      const s = findScenario("panel", state);
      if (s === undefined) {
        throw new Error(state);
      }
      const kim = (at: number, clocks = trackOutcomes(new Map(), s.home.members, 1000)) => {
        const m = panelModel(s.home, replay(s.outputs), { ...NO_LOCAL, outcomes: showingOutcomes(clocks, at) });
        return m.kind === "home" ? m.people.find((p) => p.name === "Kim Lee")?.action : undefined;
      };
      expect(kim(1000), state).toEqual({ kind: "status", label });
      expect(kim(1000 + RING_OUTCOME_MS - 1), state).toEqual({ kind: "status", label });
      expect(kim(1000 + RING_OUTCOME_MS), state).toEqual({ kind: "call", enabled: true });
      // Without a clock (never seen) it's Call, as before.
      expect(kim(1000, new Map()), state).toEqual({ kind: "call", enabled: true });
    }
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
    const closed = panelModel(s.home, initialCall, { ...NO_LOCAL, dismissed: new Set(["That isn't a TwinCursor call link."]) });
    expect(closed.kind === "home" && closed.error).toBeNull();
    const own = panelModel(s.home, initialCall, { ...NO_LOCAL, error: "Sign in first." });
    expect(own.kind === "home" && own.error).toBe("Sign in first.");
  });

  it("pins a server problem to the bottom", () => {
    expect(home("offline").serverProblem).toBe("Can't reach the server.");
  });

  it("on an invite-only server, offers Paste link first and asking to be invited instead of Share", () => {
    const m = home("uninvited");
    expect(m.main).toEqual({ kind: "invite", busy: false, primary: true, asked: false });
    expect(m.people).toHaveLength(3);
    expect(m.people.every((p) => p.action.kind === "none")).toBe(true);
    expect(home("uninvited-asked").main).toEqual({ kind: "invite", busy: false, primary: true, asked: true });
  });

  it("on an invite-only server, a ring still shows with Join and takes the primary button", () => {
    const s = findScenario("panel", "uninvited");
    if (s === undefined) {
      throw new Error("no uninvited scenario");
    }
    const sam = s.home.contacts[0];
    if (sam === undefined) {
      throw new Error("no contacts");
    }
    const m = panelModel({ ...s.home, rings: [{ call_id: "c7", host: sam }] }, initialCall);
    expect(m.kind === "home" && m.rings.map((r) => r.callId)).toEqual(["c7"]);
    expect(m.kind === "home" && m.main).toEqual({ kind: "invite", busy: false, primary: false, asked: false });
  });

  it("on an invite-only server, joining works as for anyone", () => {
    const s = findScenario("panel", "uninvited");
    if (s === undefined) {
      throw new Error("no uninvited scenario");
    }
    const starting = panelModel({ ...s.home, stage: { kind: "starting" } }, initialCall);
    expect(starting.kind === "home" && starting.main).toEqual({ kind: "invite", busy: true, primary: true, asked: false });
    const sam = s.home.contacts[0];
    if (sam === undefined) {
      throw new Error("no contacts");
    }
    const joined = panelModel({ ...s.home, stage: { kind: "joined", host: sam } }, initialCall);
    expect(joined.kind === "home" && joined.main.kind).toBe("call");
  });

  it("a host taken off the list mid-call keeps the call, without Call buttons", () => {
    const s = findScenario("panel", "hosting");
    if (s === undefined) {
      throw new Error("no hosting scenario");
    }
    const m = panelModel({ ...s.home, account: { ...s.home.account, can_host: false } }, replay(s.outputs));
    if (m.kind !== "home") {
      throw new Error(m.kind);
    }
    expect(m.main.kind === "call" && m.main.action).toEqual({ label: "Stop sharing", op: "end" });
    const byName = Object.fromEntries(m.people.map((p) => [p.name, p.action]));
    expect(byName["Sam Rivera"]).toEqual({ kind: "status", label: "In the call" });
    expect(byName["Priya Natarajan"]).toEqual({ kind: "none" });
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
