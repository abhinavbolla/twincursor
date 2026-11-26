import { describe, expect, it } from "vitest";

import { compact, MIN_NAME_PX } from "./fit";

const both = { faces: true, keys: true };

describe("fitting the bar's row", () => {
  it("leaves a row alone while the name fits, or is short and whole", () => {
    expect(compact(0, 80, 80, both)).toBe(0);
    expect(compact(0, MIN_NAME_PX, 120, both)).toBe(0);
    expect(compact(0, 30, 30, both)).toBe(0);
  });

  it("drops the faces first, then the keycaps", () => {
    expect(compact(0, 20, 90, both)).toBe(1);
    expect(compact(1, 20, 90, both)).toBe(2);
    expect(compact(2, 20, 90, both)).toBe(2);
  });

  it("skips what the row doesn't have", () => {
    expect(compact(0, 20, 90, { faces: false, keys: true })).toBe(2);
    expect(compact(0, 20, 90, { faces: true, keys: false })).toBe(1);
    expect(compact(1, 20, 90, { faces: true, keys: false })).toBe(1);
    expect(compact(0, 20, 90, { faces: false, keys: false })).toBe(0);
  });
});
