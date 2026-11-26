import { describe, expect, it } from "vitest";

import eventLines from "../../../crates/ipc/fixtures/events.jsonl?raw";
import { callReducer, endMessage, initialCall, numbersReport, readouts, statsLine, type CallState } from "./call";
import { parseEvent, parseOutput, type EngineOutput } from "./ipc";

const fixtures: unknown[] = eventLines
  .split("\n")
  .filter((l) => l.trim() !== "")
  .map((l) => JSON.parse(l) as unknown);

const run = (state: CallState, ...outputs: EngineOutput[]): CallState =>
  outputs.reduce((s, output) => callReducer(s, { type: "output", output }), state);

const started = (role: "share" | "join" = "share", codec: "h264" | "h265" = "h264"): EngineOutput => ({
  kind: "started",
  role,
  codec,
  pid: 1,
  args: ["share", "--ipc", "--room=7kq2-m9xd-p4tz-hc3w"],
});
const ev = (event: unknown): EngineOutput => ({ kind: "event", event });

describe("engine events", () => {
  it("reads every fixture event", () => {
    const kinds = new Set<string>();
    for (const value of fixtures) {
      const event = parseEvent(value);
      expect(event, JSON.stringify(value)).not.toBeNull();
      kinds.add(event?.event ?? "");
    }
    for (const kind of [
      "hello",
      "connection",
      "participants",
      "control_asked",
      "control",
      "voice",
      "stats",
      "error",
      "ended",
    ]) {
      expect(kinds.has(kind), kind).toBe(true);
    }
  });

  it("refuses what isn't an event", () => {
    expect(parseEvent({ event: "format_disk" })).toBeNull();
    expect(parseEvent({ event: "connection", state: "sideways" })).toBeNull();
    expect(parseEvent({ event: "control_asked", request: "1", identity: "x", name: "x", platform: "x" })).toBeNull();
    expect(parseEvent("hello")).toBeNull();
    expect(parseOutput({ kind: "started", role: "share", codec: "av1", pid: 1, args: [] })).toBeNull();
  });

  it("plays the whole fixture file into a sensible call", () => {
    let s = run(initialCall, started());
    for (const value of fixtures) {
      s = run(s, ev(value));
    }
    expect(s.phase).toBe("in_call");
    expect(s.engine).toBe("0.1.0");
    expect(s.connection).toBe("reconnecting");
    // The last participants event is the empty one.
    expect(s.participants).toEqual([]);
    expect(s.control).toEqual({ state: "none", by: null });
    expect(s.asked).toBeNull();
    expect(s.voice?.sending).toBe(false);
    expect(s.stats?.codec).toBe("h265");
    expect(s.error).toBe("No one is asking for control right now.");
    expect(s.ended).toBe("disconnected");
    s = run(s, { kind: "exited", code: 0 });
    expect(s.phase).toBe("idle");
    expect(s.endMessage).toBe("The call ended. The connection to the server closed.");
  });

  it("keeps a control request until it's answered or over", () => {
    const asked = fixtures.find((f) => (f as { event: string }).event === "control_asked");
    let s = run(initialCall, started(), ev(asked));
    expect(s.asked).toEqual({ request: 1, identity: "guest-1f2e3d4c", name: "Sam", platform: "macos" });
    s = run(s, ev({ event: "control", state: "asked", by: { identity: "guest-1f2e3d4c", name: "Sam" } }));
    expect(s.asked?.request).toBe(1);
    // Answering clears the prompt at once.
    expect(callReducer(s, { type: "answered" }).asked).toBeNull();
    // So does the guest withdrawing, or control starting.
    expect(run(s, ev({ event: "control", state: "none", by: null })).asked).toBeNull();
    const controlling = run(
      s,
      ev({ event: "control", state: "controlling", by: { identity: "guest-1f2e3d4c", name: "Sam" } }),
    );
    expect(controlling.asked).toBeNull();
    expect(controlling.control.by?.name).toBe("Sam");
  });

  it("warns when the engine speaks another protocol", () => {
    const s = run(initialCall, started(), ev({ event: "hello", version: 2, role: "sharer", engine: "9" }));
    expect(s.error).toMatch(/don't match/);
  });

  it("keeps the call through a switch to H.264", () => {
    let s = run(
      initialCall,
      started("share", "h265"),
      ev({ event: "connection", state: "connected" }),
      ev({
        event: "participants",
        participants: [{ identity: "guest-1", name: "Sam", role: "guest", decoders: ["h264"] }],
      }),
      { kind: "restarting", codec: "h264", message: "Sam's computer can't decode HEVC; switched to H.264." },
    );
    expect(s.phase).toBe("in_call");
    expect(s.codec).toBe("h264");
    expect(s.notice).toMatch(/switched to H.264/);
    expect(s.participants).toEqual([]);
    expect(s.connection).toBeNull();
    s = run(s, started("share", "h264"), ev({ event: "connection", state: "connected" }));
    expect(s.notice).toMatch(/switched to H.264/);
    expect(s.connection).toBe("connected");
  });

  it("explains how a call ended", () => {
    expect(endMessage("left", null, 0)).toBe("You left the call.");
    expect(endMessage("failed", "Sharing failed: no route.", 1)).toBe("The call stopped: Sharing failed: no route.");
    expect(endMessage(null, null, 101)).toBe("The engine stopped unexpectedly (exit code 101).");
    expect(endMessage(null, null, null)).toBe("The engine stopped unexpectedly.");
    let s = run(initialCall, started(), ev({ event: "ended", reason: "left" }), { kind: "exited", code: 0 });
    expect(s.endMessage).toBe("You left the call.");
    s = callReducer(s, { type: "dismiss" });
    expect(s.endMessage).toBeNull();
  });

  it("keeps the last few hundred log lines across calls", () => {
    let s = run(initialCall, started());
    for (let i = 0; i < 350; i++) {
      s = run(s, { kind: "log", line: `line ${i}` });
    }
    expect(s.logs).toHaveLength(300);
    expect(s.logs[299]).toBe("line 349");
    s = run(s, { kind: "exited", code: 0 }, started());
    expect(s.logs).toHaveLength(300);
  });

  it("writes the numbers testers paste", () => {
    const stats = {
      width: 1920,
      height: 1080,
      fps: 59.84,
      kbps: 2480.5,
      rtt_ms: 4.2,
      codec: "h264",
      shown_pct: 100,
    };
    expect(statsLine(stats, "join")).toBe("1920×1080 · 59.8 fps · 2481 kbps · round trip 4 ms · H.264 · shown 1:1");
    expect(statsLine({ ...stats, rtt_ms: null, codec: "h265", shown_pct: null }, "share")).toBe(
      "1920×1080 · 59.8 fps · 2481 kbps · round trip – · HEVC",
    );
    expect(statsLine(null, "share")).toBe("Not sending video yet.");
    expect(readouts(stats, "join").map((r) => `${r.label} ${r.value}${r.unit ?? ""}`)).toEqual([
      "Resolution 1920×1080",
      "Frame rate 59.8fps",
      "Bitrate 2481kbps",
      "Round trip 4ms",
      "Codec H.264",
      "Shown at 1:1",
    ]);
    expect(readouts({ ...stats, rtt_ms: null, codec: null }, "share").map((r) => r.value)).toEqual([
      "1920×1080",
      "59.8",
      "2481",
      "–",
    ]);
    const s = run(initialCall, started("join"), ev({ event: "stats", ...stats }));
    const report = numbersReport(s, new Date("2026-11-19T10:00:00Z"));
    expect(report).toContain("1920x1080, 59.8 fps, 2481 kbps, round trip 4 ms, codec h264, shown 100%");
    expect(report).toContain("role: join");
  });
});
