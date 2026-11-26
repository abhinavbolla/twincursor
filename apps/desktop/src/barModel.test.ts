import { describe, expect, it } from "vitest";

import { barModel, type BarModel } from "./barModel";
import { initialCall } from "./call";
import { parseHomeView } from "./home";
import { parseOutput } from "./ipc";
import { phraseText } from "./people";
import { findScenario, homeView, scenariosOf } from "./scenarios";
import { replay } from "./useSurface";

type Call = Extract<BarModel, { kind: "call" }>;

function bar(state: string, local?: Parameters<typeof barModel>[2]): Call {
  const s = findScenario("bar", state);
  if (s === undefined) {
    throw new Error(`no bar scenario ${state}`);
  }
  const m = barModel(s.home, replay(s.outputs), local);
  if (m.kind !== "call") {
    throw new Error(`${state} shows no call`);
  }
  return m;
}

/** The row as a person reads it: the line, then the buttons in it, left to right. */
function row(m: Call): string[] {
  return [
    phraseText(m.line),
    ...(m.keys ?? []),
    ...(m.button === null ? [] : [`[${m.button.label}${m.button.tone === "danger" ? "!" : ""}]`]),
    ...(m.mute === null ? [] : [`(${m.mute.label})`]),
    ...(m.hasMenu ? ["(More)"] : []),
    `[${m.leave.label}]`,
  ];
}

describe("bar scenarios", () => {
  it("has every state the dev page promises", () => {
    const names = scenariosOf("bar").map((s) => s.name);
    for (const name of [
      "alone",
      "knock",
      "knock2",
      "with",
      "trusted",
      "controlling",
      "asked",
      "needs",
      "menu",
      "notice",
      "g-knocking",
      "g-watching",
      "g-asked",
      "g-controlling",
      "reconnecting",
      "muted",
    ]) {
      expect(names).toContain(name);
    }
  });

  it("are what the Rust side and the engine could send", () => {
    for (const s of scenariosOf("bar")) {
      expect(parseHomeView(JSON.parse(JSON.stringify(s.home))), s.name).toEqual(s.home);
      for (const o of s.outputs) {
        expect(parseOutput(JSON.parse(JSON.stringify(o))), s.name).toEqual(o);
      }
    }
  });

  it("all show a call", () => {
    for (const s of scenariosOf("bar")) {
      expect(barModel(s.home, replay(s.outputs)).kind, s.name).toBe("call");
    }
  });
});

describe("the bar's row", () => {
  it.each([
    ["alone", ["Sharing your screen", "[Copy link]", "(Mute)", "(More)", "[Stop]"]],
    ["knock", ["Sharing your screen", "[Copy link]", "(Mute)", "(More)", "[Stop]"]],
    ["with", ["With Sam", "(Mute)", "(More)", "[Stop]"]],
    ["trusted", ["Sam can take control", "[Take back]", "(Mute)", "(More)", "[Stop]"]],
    ["controlling", ["Sam is controlling", "⌃", "⌥", "⌘", "R", "[Take back!]", "(Mute)", "[Stop]"]],
    ["asked", ["With Sam", "(Mute)", "(More)", "[Stop]"]],
    ["menu", ["Sam can take control", "[Take back]", "(Mute)", "(More)", "[Stop]"]],
    ["notice", ["With Sam and Kim", "(Mute)", "(More)", "[Stop]"]],
    ["reconnecting", ["Reconnecting…", "(Mute)", "(More)", "[Stop]"]],
    ["muted", ["With Sam", "(Unmute)", "(More)", "[Stop]"]],
    ["g-knocking", ["Waiting for Alex to let you in…", "[Cancel]"]],
    ["g-watching", ["Alex's screen", "[Control]", "(Mute)", "(More)", "[Leave]"]],
    ["g-asked", ["Asked Alex for control…", "[Never mind]", "(Mute)", "(More)", "[Leave]"]],
    ["g-controlling", ["Controlling Alex's Mac", "F9", "[Give back]", "(Mute)", "[Leave]"]],
  ])("%s", (state, expected) => {
    expect(row(bar(state))).toEqual(expected);
  });

  it("is loud only while someone controls the sharer's Mac", () => {
    for (const s of scenariosOf("bar")) {
      expect(bar(s.name).live, s.name).toBe(s.name === "controlling");
    }
  });

  it("names people by their first name, and their names are what truncates", () => {
    expect(bar("controlling").line).toEqual([{ name: "Sam" }, " is controlling"]);
    expect(bar("g-knocking").line).toEqual(["Waiting for ", { name: "Alex" }, " to let you in…"]);
  });

  it("shows the connection as a dot", () => {
    expect(bar("with").dot).toBe("good");
    expect(bar("reconnecting").dot).toBe("pending");
    expect(bar("g-knocking").dot).toBe("off");
  });

  it("shows who is in the call, sharer first, and drops the faces and the menu while someone is in control", () => {
    expect([bar("controlling").hasMenu, bar("g-controlling").hasMenu, bar("trusted").hasMenu]).toEqual([false, false, true]);
    expect(bar("alone").faces).toEqual([]);
    expect(bar("with").faces.map((f) => f.name)).toEqual(["Sam Rivera"]);
    expect(bar("g-watching").faces.map((f) => f.name)).toEqual(["Alex Morgan"]);
    expect(bar("g-knocking").faces.map((f) => f.name)).toEqual(["Alex Morgan"]);
    expect(bar("controlling").faces).toEqual([]);
    expect(bar("g-controlling").faces).toEqual([]);
  });

  it("shows three faces at most, then +N", () => {
    const s = findScenario("bar", "with");
    if (s === undefined) {
      throw new Error("no with scenario");
    }
    const guests = ["a", "b", "c", "d", "e"].map((id) => ({ identity: `guest-${id}`, name: `Guest ${id}`, role: "guest", decoders: [] }));
    const call = replay([...s.outputs, { kind: "event", event: { event: "participants", participants: guests } }]);
    const m = barModel(s.home, call);
    expect(m.kind === "call" && [m.faces.length, m.extra]).toEqual([3, 2]);
    expect(m.kind === "call" && phraseText(m.line)).toBe("With Guest and 4 others");
  });

  it("can't mute while voice is off or not there yet", () => {
    const s = findScenario("bar", "alone");
    if (s === undefined) {
      throw new Error("no alone scenario");
    }
    const noVoice = replay(s.outputs.filter((o) => !(o.kind === "event" && (o.event as { event: string }).event === "voice")));
    const m = barModel(s.home, noVoice);
    expect(m.kind === "call" && m.mute).toEqual({ state: "off", label: "Voice is connecting" });
  });

  it("disables Stop while the call ends", () => {
    const m = barModel(homeView({ stage: { kind: "ending" } }), initialCall);
    expect(m.kind === "call" && [phraseText(m.line), m.leave.enabled, m.mute]).toEqual(["Ending the call…", false, null]);
  });

  it("shows nothing outside a call", () => {
    expect(barModel(null, initialCall)).toEqual({ kind: "none" });
    expect(barModel(homeView(), initialCall)).toEqual({ kind: "none" });
  });
});

describe("the bar's sections", () => {
  it("shows a knock, with Let in off until it's armed", () => {
    expect(bar("knock").knocks).toEqual([{ userId: "u2", name: "Sam Rivera", handle: "samrivera", avatar: null, armed: false }]);
    expect(bar("knock2").knocks.map((k) => [k.name, k.armed])).toEqual([
      ["Sam Rivera", true],
      ["Bartholomew Montgomery-Fitzgerald Junior", true],
    ]);
    expect(bar("g-knocking").knocks).toEqual([]);
  });

  it("shows a control request to the sharer only", () => {
    expect(bar("asked").ask).toEqual({ request: 1, name: "Sam Rivera", avatar: null });
    expect(bar("with").ask).toBeNull();
    expect(bar("g-asked").ask).toBeNull();
  });

  it("asks for Accessibility", () => {
    expect(bar("needs").needsAccessibility).toBe(true);
    expect(bar("with").needsAccessibility).toBe(false);
  });

  it("shows at most two notices, errors first, and not the closed ones", () => {
    expect(bar("notice").notices).toEqual([
      { text: "Kim Lee's computer can't decode HEVC; switched to H.264.", tone: "plain" },
      { text: "The call link is on your clipboard.", tone: "plain" },
    ]);
    expect(bar("notice", { dismissed: new Set(["The call link is on your clipboard."]), error: null }).notices).toHaveLength(1);
    const withError = bar("notice", { dismissed: new Set(), error: "Too soon. Try again." }).notices;
    expect(withError).toEqual([
      { text: "Too soon. Try again.", tone: "bad" },
      { text: "Kim Lee's computer can't decode HEVC; switched to H.264.", tone: "plain" },
    ]);
  });

  it("has the link, the numbers, a Give control row per guest and the mic in its menu", () => {
    const menu = bar("menu").menu;
    expect(menu.link).toBe("twincursor://call/0123456789abcdef0123456789abcdef");
    expect(menu.numbers?.map((r) => `${r.label} ${r.value}${r.unit === null ? "" : ` ${r.unit}`}`)).toEqual([
      "Resolution 3840×2160",
      "Frame rate 59.8 fps",
      "Bitrate 9184 kbps",
      "Round trip 4 ms",
      "Codec HEVC",
    ]);
    expect(menu.guests.map((g) => [g.name, g.state])).toEqual([
      ["Sam Rivera", "trusted"],
      ["Kim Lee", "give"],
    ]);
    expect(menu.voice).toBe("Your mic: MacBook Pro Microphone");
    expect(menu.muteKeys).toEqual(["⌃", "⌥", "⌘", "M"]);
    expect(bar("controlling").menu.guests.map((g) => g.state)).toEqual(["controlling"]);
    expect(bar("muted").menu.voice).toBe("Your mic: MacBook Pro Microphone (muted)");
  });

  it("gives a guest's menu no link, no Give control and no global hotkey", () => {
    const menu = bar("g-watching").menu;
    expect([menu.link, menu.guests, menu.muteKeys]).toEqual([null, [], null]);
    expect(menu.numbers?.map((r) => r.label)).toContain("Shown at");
  });

  it("says when there are no numbers yet", () => {
    const m = barModel(homeView({ stage: { kind: "hosting", call_id: "c", link: "l" } }), initialCall);
    expect(m.kind === "call" && [m.menu.numbers, m.menu.numbersEmpty]).toEqual([null, "Not sending video yet."]);
  });
});
