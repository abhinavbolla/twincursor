import { describe, expect, it } from "vitest";

import { blockers, needs, parsePermissions } from "./permissions";

describe("permissions", () => {
  it("reads the engine's report", () => {
    const status = parsePermissions({
      screen_recording: "granted",
      accessibility: "not_granted",
      microphone: "not_asked",
      pid: 2,
      parent_pid: 1,
      executable: "/Applications/TwinCursor.app/Contents/MacOS/tc-engine",
    });
    expect(status).toEqual({
      screen_recording: "granted",
      accessibility: "not_granted",
      microphone: "not_asked",
    });
    expect(parsePermissions({ screen_recording: "maybe" })).toBeNull();
  });

  it("say what each role needs", () => {
    expect(needs(true).share.map((n) => n.name)).toEqual(["screen_recording", "accessibility", "microphone"]);
    expect(needs(false).share.map((n) => n.name)).toEqual(["screen_recording", "accessibility"]);
    expect(needs(true).join.map((n) => n.name)).toEqual(["microphone"]);
    expect(needs(false).join).toEqual([]);
  });

  it("never block joining on what only sharing needs", () => {
    const none = { screen_recording: "denied", accessibility: "denied", microphone: "denied" } as const;
    expect(blockers("join", none)).toEqual([]);
    expect(blockers("share", none)).toEqual(["screen_recording"]);
    expect(blockers("share", { ...none, screen_recording: "granted" })).toEqual([]);
    expect(blockers("share", null)).toEqual([]);
  });
});
