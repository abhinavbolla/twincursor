import { describe, expect, it } from "vitest";

import { due, forget, nextDue, NOTICE_MS, pause, resume, track } from "./notices";

const link = { text: "The call link is on your clipboard.", tone: "plain" } as const;
const error = { text: "Too soon. Try again.", tone: "bad" } as const;

describe("notice clocks", () => {
  it("lets a notice go after six seconds, and never an error", () => {
    const clocks = track(new Map(), [error, link], 1000);
    expect([...clocks.keys()]).toEqual([link.text]);
    expect(due(clocks, 1000 + NOTICE_MS - 1)).toEqual([]);
    expect(due(clocks, 1000 + NOTICE_MS)).toEqual([link.text]);
    expect(nextDue(clocks, 2000)).toBe(NOTICE_MS - 1000);
  });

  it("keeps a notice's clock while it shows, and drops it once it's gone", () => {
    const first = track(new Map(), [link], 0);
    const later = track(first, [link], 4000);
    expect(due(later, NOTICE_MS)).toEqual([link.text]);
    expect(track(later, [], 5000).size).toBe(0);
    // Shown again later: a fresh six seconds.
    expect(due(track(new Map(), [link], 9000), 9000 + NOTICE_MS - 1)).toEqual([]);
  });

  it("pauses while the pointer is over it", () => {
    let clocks = track(new Map(), [link], 0);
    clocks = pause(clocks, link.text, 2000);
    expect(due(clocks, 60_000)).toEqual([]);
    expect(nextDue(clocks, 60_000)).toBeNull();
    clocks = resume(clocks, link.text, 10_000);
    // Two seconds had passed before the pause; four are left.
    expect(due(clocks, 10_000 + 3999)).toEqual([]);
    expect(due(clocks, 10_000 + 4000)).toEqual([link.text]);
  });

  it("ignores a pause or resume for a line without a clock", () => {
    const clocks = track(new Map(), [error], 0);
    expect(pause(clocks, error.text, 1).size).toBe(0);
    expect(resume(clocks, error.text, 1).size).toBe(0);
  });

  it("forgets closed lines that are no longer sent", () => {
    const closed = new Set([link.text, error.text]);
    expect(forget(closed, [link.text, null])).toEqual(new Set([link.text]));
    expect(forget(closed, [link.text, error.text])).toBe(closed);
  });
});
