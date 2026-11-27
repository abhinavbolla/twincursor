import { describe, expect, it } from "vitest";

import { initialCall } from "./call";
import { panelModel } from "./panelModel";
import { CANT_SHARE_LINE, MAC, WINDOWS, osOfUserAgent, parsePlatform, platformOfParams, signInLine } from "./platform";
import { findScenario, scenariosOf } from "./scenarios";
import { replay } from "./useSurface";

describe("the platform answer", () => {
  it("is read only when it has both fields", () => {
    expect(parsePlatform({ os: "windows", can_share: false })).toEqual(WINDOWS);
    expect(parsePlatform({ os: "macos", can_share: true })).toEqual(MAC);
    expect(parsePlatform({ os: "windows" })).toBeNull();
    expect(parsePlatform("windows")).toBeNull();
    expect(parsePlatform(null)).toBeNull();
  });

  it("comes from the dev page's URL", () => {
    expect(platformOfParams(new URLSearchParams("platform=windows"))).toEqual(WINDOWS);
    expect(platformOfParams(new URLSearchParams("platform=macos"))).toEqual(MAC);
    expect(platformOfParams(new URLSearchParams(""))).toBeNull();
  });

  it("names the OS from a webview's user agent", () => {
    const edge =
      "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36 Edg/140.0.0.0";
    const webkit = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko)";
    expect(osOfUserAgent(edge)).toBe("windows");
    expect(osOfUserAgent(webkit)).toBe("macos");
    expect(osOfUserAgent("Mozilla/5.0 (X11; Linux x86_64)")).toBe("other");
  });

  it("says what signing in is for", () => {
    expect(signInLine(MAC)).toBe("Sign in to share your screen and call people.");
    expect(signInLine(WINDOWS)).toBe("Sign in to join calls with a link.");
  });
});

describe("the panel on Windows", () => {
  const onWindows = (state: string) => {
    const s = findScenario("panel", state);
    if (s === undefined) {
      throw new Error(state);
    }
    const m = panelModel(s.home, replay(s.outputs), undefined, WINDOWS);
    if (m.kind !== "home") {
      throw new Error(`${state} is ${m.kind}`);
    }
    return m;
  };

  it("says sharing isn't there and makes Paste link the main action", () => {
    expect(onWindows("home").main).toEqual({ kind: "join", note: CANT_SHARE_LINE, busy: false, primary: true });
    expect(CANT_SHARE_LINE).toBe("Sharing from Windows isn't available yet. You can join a call with a link.");
  });

  it("lists people without Call buttons", () => {
    const m = onWindows("many");
    expect(m.people.length).toBe(12);
    expect(m.people.every((p) => p.action.kind === "none")).toBe(true);
  });

  it("still shows rings, which take the primary button", () => {
    const m = onWindows("ring");
    expect(m.rings.map((r) => r.name)).toEqual(["Sam Rivera"]);
    expect(m.main).toEqual({ kind: "join", note: CANT_SHARE_LINE, busy: false, primary: false });
  });

  it("never asks for Screen Recording", () => {
    expect(onWindows("needs").needsScreenRecording).toBe(false);
  });

  it("shows a guest's call as on a Mac", () => {
    for (const state of ["knocking", "joined"]) {
      const s = findScenario("panel", state);
      if (s === undefined) {
        throw new Error(state);
      }
      const call = replay(s.outputs);
      expect(panelModel(s.home, call, undefined, WINDOWS), state).toEqual(panelModel(s.home, call, undefined, MAC));
    }
  });

  it("never offers Share or Call in any panel scenario", () => {
    for (const s of scenariosOf("panel")) {
      const m = panelModel(s.home, replay(s.outputs), undefined, WINDOWS);
      if (m.kind !== "home") {
        continue;
      }
      expect(m.main.kind, s.name).not.toBe("share");
      expect(
        m.people.some((p) => p.action.kind === "call"),
        s.name,
      ).toBe(false);
    }
  });

  it("puts the platform before the invite list: no Ask to be invited on Windows", () => {
    for (const state of ["uninvited", "uninvited-asked"]) {
      const m = onWindows(state);
      expect(m.main, state).toEqual({ kind: "join", note: CANT_SHARE_LINE, busy: false, primary: true });
      expect(m.people.every((p) => p.action.kind === "none")).toBe(true);
    }
    // On a Mac the invite rule applies as before.
    const s = findScenario("panel", "uninvited");
    if (s === undefined) {
      throw new Error("uninvited");
    }
    const mac = panelModel(s.home, initialCall, undefined, MAC);
    expect(mac.kind === "home" && mac.main).toEqual({ kind: "invite", busy: false, primary: true, asked: false });
  });

  it("is a Mac's panel unless told otherwise", () => {
    const s = findScenario("panel", "home");
    if (s === undefined) {
      throw new Error("home");
    }
    expect(panelModel(s.home, initialCall)).toEqual(panelModel(s.home, initialCall, undefined, MAC));
  });
});
