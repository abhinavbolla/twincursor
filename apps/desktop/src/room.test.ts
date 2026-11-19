import { describe, expect, it } from "vitest";

import { CODE_BITS, makeRoomCode, normalizeRoomCode } from "./room";

describe("room codes", () => {
  it("carry at least 80 random bits, in four readable groups", () => {
    expect(CODE_BITS).toBeGreaterThanOrEqual(80);
    const code = makeRoomCode();
    expect(code).toMatch(/^[0-9a-hjkmnp-tv-z]{4}(-[0-9a-hjkmnp-tv-z]{4}){3}$/);
    expect(makeRoomCode()).not.toBe(code);
  });

  it("use every bit of the randomness they're given", () => {
    const zeros = makeRoomCode((n) => new Uint8Array(n));
    expect(zeros).toBe("0000-0000-0000-0000");
    const top = makeRoomCode((n) => new Uint8Array(n).fill(255));
    expect(top).toBe("zzzz-zzzz-zzzz-zzzz");
    // Only the low five bits count, so every byte value maps evenly.
    const seq = makeRoomCode((n) => Uint8Array.from({ length: n }, (_, i) => i + 32));
    expect(seq).toBe("0123-4567-89ab-cdef");
  });

  it("read pasted codes forgivingly, and refuse others", () => {
    expect(normalizeRoomCode(" 7KQ2 M9XD-P4TZ-HC3W\n")).toBe("7kq2-m9xd-p4tz-hc3w");
    expect(normalizeRoomCode("7kq2m9xdp4tzhc3w")).toBe("7kq2-m9xd-p4tz-hc3w");
    expect(normalizeRoomCode("okq2-m9xd-p4tz-hcIL")).toBe("0kq2-m9xd-p4tz-hc11");
    expect(normalizeRoomCode("")).toBeNull();
    expect(normalizeRoomCode("abc")).toBeNull();
    expect(normalizeRoomCode("7kq2-m9xd-p4tz-hc3u")).toBeNull();
    expect(normalizeRoomCode("7kq2-m9xd-p4tz-hc3w-0000")).toBeNull();
  });
});
