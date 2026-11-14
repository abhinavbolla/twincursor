import { describe, expect, it } from "vitest";

import {
  captureChecks,
  parseReport,
  permissionChecks,
  PROBE_TITLE,
  stdinCloseCheck,
  type CaptureReport,
} from "./checks";

// Shaped like the output of `tc-spike permissions --capture --exclude-app-pid …`.
const status = {
  screen_recording: "granted",
  accessibility: "not_granted",
  microphone: "not_asked",
  pid: 200,
  parent_pid: 100,
  executable: "/Applications/TwinCursor.app/Contents/MacOS/tc-spike",
};

function probe(id: number, share: number, late = false) {
  return { id, title: PROBE_TITLE, opened_after_start: late, probe_color: share };
}

function captureReport(over: Partial<CaptureReport> = {}): CaptureReport {
  return {
    frame: [1504, 982],
    error: null,
    included: [probe(1, 0.98), { id: 9, title: "TwinCursor", opened_after_start: false, probe_color: 0 }],
    excluded: [probe(1, 0), probe(2, 0, true)],
    ...over,
  };
}

describe("parseReport", () => {
  it("reads the helper's status", () => {
    const report = parseReport(status);
    expect(report.screen_recording).toBe("granted");
    expect(report.capture).toBeUndefined();
  });

  it("reads a capture result", () => {
    const report = parseReport({ ...status, capture: captureReport() });
    expect(report.capture?.frame).toEqual([1504, 982]);
    expect(report.capture?.excluded[1]?.opened_after_start).toBe(true);
  });

  it("rejects what it doesn't know", () => {
    expect(() => parseReport({ ...status, microphone: "maybe" })).toThrow();
    expect(() => parseReport("hello")).toThrow();
    expect(() => parseReport({ ...status, pid: "1" })).toThrow();
  });
});

describe("permissionChecks", () => {
  it("passes only what is granted", () => {
    const verdicts = permissionChecks(parseReport(status)).map((c) => c.verdict);
    expect(verdicts).toEqual(["pass", "fail", "fail"]);
  });
});

describe("captureChecks", () => {
  it("passes when the probes are seen without the exclusion and gone with it", () => {
    expect(captureChecks(captureReport()).map((c) => c.verdict)).toEqual(["pass", "pass", "pass"]);
  });

  it("fails a window that leaks into the capture", () => {
    const leak = captureReport({ excluded: [probe(1, 0), probe(2, 0.97, true)] });
    expect(captureChecks(leak).map((c) => c.verdict)).toEqual(["pass", "pass", "fail"]);
  });

  it("can't pass when the probe wasn't visible to begin with", () => {
    const blind = captureReport({ included: [probe(1, 0.1)] });
    expect(captureChecks(blind).map((c) => c.verdict)).toEqual(["pass", "fail", "fail"]);
  });

  it("fails the late check when no window opened during capture", () => {
    const none = captureReport({ excluded: [probe(1, 0)] });
    const [, , late] = captureChecks(none);
    expect(late?.verdict).toBe("fail");
    expect(late?.detail).toMatch(/no window/);
  });

  it("reports a capture error", () => {
    const checks = captureChecks(captureReport({ frame: null, error: "Screen Recording isn't allowed" }));
    expect(checks.every((c) => c.verdict === "fail")).toBe(true);
    expect(checks[0]?.detail).toMatch(/Screen Recording/);
    expect(captureChecks(undefined)[0]?.verdict).toBe("fail");
  });
});

describe("stdinCloseCheck", () => {
  it("passes a quick exit and fails a slow or missing one", () => {
    expect(stdinCloseCheck({ exited_after_ms: 40 }).verdict).toBe("pass");
    expect(stdinCloseCheck({ exited_after_ms: 5000 }).verdict).toBe("fail");
    expect(stdinCloseCheck({ exited_after_ms: null }).verdict).toBe("fail");
  });
});
