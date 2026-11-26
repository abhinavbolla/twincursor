import { describe, expect, it } from "vitest";

import { fitHeight, measure, POPOVER_MARGIN, SHOWN_FOCUS_MS, unasked, viewOf } from "./inWindow";

/** Just enough of an element for `measure`: its box, and open popovers' boxes. */
function element(height: number, loading = false, menus: { top: number; bottom: number }[] = []): HTMLElement {
  return {
    querySelector: (selector: string) => (loading && selector === ".panel-loading" ? {} : null),
    querySelectorAll: () => menus.map((box) => ({ getBoundingClientRect: () => box })),
    getBoundingClientRect: () => ({ top: 0, bottom: height, height }),
  } as unknown as HTMLElement;
}

describe("the window's page", () => {
  it("shows the bar for view=bar and the panel otherwise", () => {
    expect(viewOf(new URLSearchParams("?view=bar"))).toBe("bar");
    expect(viewOf(new URLSearchParams("?view=panel"))).toBe("panel");
    expect(viewOf(new URLSearchParams(""))).toBe("panel");
    expect(viewOf(new URLSearchParams("?view=gallery"))).toBe("panel");
  });

  it("measures the surface in whole points, rounding up so nothing is cut off", () => {
    expect(measure(element(48))).toBe(48);
    expect(measure(element(212.2))).toBe(213);
  });

  it("grows to an open menu's bottom edge, with a margin under it, and back when it closes", () => {
    // The signed-out panel is about 150 points; its account menu hangs to 196.
    expect(measure(element(150, false, [{ top: 44, bottom: 196 }]))).toBe(196 + POPOVER_MARGIN);
    expect(measure(element(150))).toBe(150);
    // A menu that ends inside a tall panel changes nothing.
    expect(measure(element(480, false, [{ top: 44, bottom: 196 }]))).toBe(480);
  });

  it("measures from the surface's top, wherever the page puts it", () => {
    expect(fitHeight({ top: 24, bottom: 174 }, [{ top: 68, bottom: 220.4 }])).toBe(Math.ceil(220.4 + POPOVER_MARGIN - 24));
    expect(fitHeight({ top: 24, bottom: 24 }, [{ top: 68, bottom: 220 }])).toBeNull();
  });

  it("doesn't fit a window to nothing or to the loading placeholder", () => {
    expect(measure(element(0))).toBeNull();
    expect(measure(element(200, true))).toBeNull();
  });
});

describe("focus when the window shows", () => {
  it("drops the focus WebKit gives the first button as the window becomes key", () => {
    expect(unasked(1000, 0, 1000)).toBe(true);
    expect(unasked(1000, 0, 1000 + SHOWN_FOCUS_MS)).toBe(true);
    // Shown right after the page loaded, never touched.
    expect(unasked(180, Number.NEGATIVE_INFINITY, 181)).toBe(true);
  });

  it("keeps focus a click or key press asked for", () => {
    // Clicked into the panel: the click comes with (or just before) the window becoming key.
    expect(unasked(1000, 990, 1005)).toBe(false);
    // Tabbing later, long after it showed.
    expect(unasked(1000, 0, 1000 + SHOWN_FOCUS_MS + 1)).toBe(false);
  });
});
