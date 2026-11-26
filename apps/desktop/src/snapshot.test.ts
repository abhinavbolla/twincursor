// A window opened in the middle of a call replays `call_snapshot` and must end up where a window
// that heard everything from the start is. The Rust side cuts a recorded call (made of the ipc
// fixture events, a restart and an exit) down to a snapshot after every output
// (`src-tauri/src/snapshot.rs`); this replays each one.

import { describe, expect, it } from "vitest";

import snapshotLines from "../src-tauri/fixtures/call-snapshots.jsonl?raw";
import streamLines from "../src-tauri/fixtures/call-stream.jsonl?raw";
import { callReducer, initialCall, type CallState } from "./call";
import { parseOutput, type EngineOutput } from "./ipc";

const lines = (text: string): unknown[] =>
  text
    .split("\n")
    .filter((l) => l.trim() !== "")
    .map((l) => JSON.parse(l) as unknown);

const outputs = (values: unknown[]): EngineOutput[] =>
  values.map((v) => {
    const output = parseOutput(v);
    if (output === null) {
      throw new Error(`not an output: ${JSON.stringify(v)}`);
    }
    return output;
  });

const replay = (list: EngineOutput[]): CallState =>
  list.reduce((s, output) => callReducer(s, { type: "output", output }), initialCall);

const stream = outputs(lines(streamLines));
const snapshots = lines(snapshotLines).map((s) => outputs(s as unknown[]));

describe("call snapshots", () => {
  it("has one snapshot for every point of the recorded call", () => {
    expect(stream.length).toBeGreaterThan(40);
    expect(snapshots).toHaveLength(stream.length);
    // The recording has what matters: a restart, control requests, an error and an exit.
    const kinds = new Set(stream.map((o) => (o.kind === "event" ? `event:${String((o.event as { event: string }).event)}` : o.kind)));
    for (const kind of ["restarting", "exited", "log", "event:control_asked", "event:error", "event:hello"]) {
      expect(kinds.has(kind), kind).toBe(true);
    }
  });

  it("brings a late window to the same state as one that listened from the start", () => {
    for (let i = 0; i < stream.length; i++) {
      const live = replay(stream.slice(0, i + 1));
      const late = replay(snapshots[i] ?? []);
      expect(late, `after output ${i}`).toEqual(live);
    }
  });

  it("is shorter than the call", () => {
    const last = snapshots[snapshots.length - 1] ?? [];
    expect(last.length).toBeLessThan(stream.length);
  });
});
