import { describe, expect, it } from "vitest";

import type { MemberState } from "./home";
import { nextOutcomeDue, RING_OUTCOME_MS, showingOutcomes, trackOutcomes } from "./ringOutcomes";

const kim = (state: MemberState) => [{ user_id: "kim", state }];

describe("ring outcomes", () => {
  it("show for 10 s from when they are first seen", () => {
    expect(RING_OUTCOME_MS).toBe(10_000);
    const clocks = trackOutcomes(new Map(), kim("missed"), 500);
    expect(showingOutcomes(clocks, 500)).toEqual(new Map([["kim", "missed"]]));
    expect(nextOutcomeDue(clocks, 500)).toBe(RING_OUTCOME_MS);
    expect(showingOutcomes(clocks, 500 + RING_OUTCOME_MS - 1).size).toBe(1);
    expect(showingOutcomes(clocks, 500 + RING_OUTCOME_MS).size).toBe(0);
    expect(nextOutcomeDue(clocks, 500 + RING_OUTCOME_MS)).toBeNull();
  });

  it("keep their clock while the state stays, even after running out", () => {
    let clocks = trackOutcomes(new Map(), kim("declined"), 0);
    clocks = trackOutcomes(clocks, kim("declined"), 4000);
    expect(clocks.get("kim")).toEqual({ outcome: "declined", seenAt: 0 });
    clocks = trackOutcomes(clocks, kim("declined"), 60_000);
    expect(showingOutcomes(clocks, 60_000).size).toBe(0);
  });

  it("start again for a new ring, and a changed outcome", () => {
    let clocks = trackOutcomes(new Map(), kim("missed"), 0);
    clocks = trackOutcomes(clocks, kim("ringing"), 20_000);
    expect(clocks.size).toBe(0);
    clocks = trackOutcomes(clocks, kim("missed"), 30_000);
    expect(showingOutcomes(clocks, 30_000)).toEqual(new Map([["kim", "missed"]]));
    clocks = trackOutcomes(clocks, kim("declined"), 31_000);
    expect(clocks.get("kim")).toEqual({ outcome: "declined", seenAt: 31_000 });
  });

  it("ignore every other state", () => {
    for (const state of ["ringing", "knocking", "admitted", "turned_away", "left", "unknown"] as const) {
      expect(trackOutcomes(new Map(), kim(state), 0).size, state).toBe(0);
    }
  });
});
