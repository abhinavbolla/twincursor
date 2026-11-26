// The page inside one of the app's native windows (the panel or the call bar). The window
// brings the rounded corners, the hairline and the shadow, so the surface fills it edge to edge
// (`.in-window` in styles.css), and the window is as tall as the surface: every change in its
// height goes to the Rust side, which resizes the window keeping its top edge (`fit_window`).

import { invoke } from "@tauri-apps/api/core";
import { useEffect, useRef, type RefObject } from "react";

export type WindowView = "panel" | "bar";

/** Which surface a page shows: `?view=bar` is the call bar, anything else the panel. */
export function viewOf(params: URLSearchParams): WindowView {
  return params.get("view") === "bar" ? "bar" : "panel";
}

/** Marks the page as running in a window, before the first paint. */
export function markInWindow(view: WindowView): void {
  document.documentElement.classList.add("in-window");
  document.documentElement.dataset.view = view;
}

/** The surface's height in whole points, or null while it has nothing to show yet. */
export function measure(el: HTMLElement): number | null {
  // The panel's placeholder before its first answer: not worth fitting a window to.
  if (el.querySelector(".panel-loading") !== null) {
    return null;
  }
  const height = Math.ceil(el.getBoundingClientRect().height);
  return height > 0 ? height : null;
}

/** How close to the window becoming key a focus counts as WebKit's, not the person's. */
export const SHOWN_FOCUS_MS = 250;

/**
 * Whether a focus at `now` is the one WebKit gives the first button when the window becomes
 * key (at `shownAt`), rather than one a click or key press (last at `touchedAt`) asked for.
 */
export function unasked(shownAt: number, touchedAt: number, now: number): boolean {
  const justShown = now - shownAt <= SHOWN_FOCUS_MS;
  const touchedSince = touchedAt >= shownAt - SHOWN_FOCUS_MS;
  return justShown && !touchedSince;
}

/**
 * Fits the window to the element the returned ref is put on. Also: nothing is focused when the
 * window shows or after it loses focus (so a stray Enter or Space can't press a button), and
 * Escape hides the panel.
 */
export function useWindowFit(view: WindowView): RefObject<HTMLDivElement | null> {
  const root = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const el = root.current;
    if (el === null) {
      return;
    }
    let last: number | null = null;
    const report = () => {
      const height = measure(el);
      if (height !== null && height !== last) {
        last = height;
        invoke("fit_window", { height }).catch(() => undefined);
      }
    };
    const observer = new ResizeObserver(report);
    observer.observe(el);
    // The placeholder going away doesn't always change the height.
    const changes = new MutationObserver(report);
    changes.observe(el, { childList: true, subtree: true });
    report();
    return () => {
      observer.disconnect();
      changes.disconnect();
    };
  }, []);

  useEffect(() => {
    const unfocus = () => {
      const active = document.activeElement;
      if (active instanceof HTMLElement) {
        active.blur();
      }
    };
    // When the window becomes key, WebKit puts focus on the first button, as if tabbed into.
    // Focus that no click or key press asked for is dropped.
    let shownAt = Number.NEGATIVE_INFINITY;
    let touchedAt = Number.NEGATIVE_INFINITY;
    const shown = () => {
      shownAt = performance.now();
      if (unasked(shownAt, touchedAt, shownAt)) {
        unfocus();
      }
    };
    const touched = () => {
      touchedAt = performance.now();
    };
    const focused = () => {
      if (unasked(shownAt, touchedAt, performance.now())) {
        unfocus();
      }
    };
    window.addEventListener("focus", shown);
    document.addEventListener("pointerdown", touched, true);
    document.addEventListener("keydown", touched, true);
    document.addEventListener("focusin", focused);
    const onKey = (e: KeyboardEvent) => {
      // A menu that closes on Escape takes it first.
      if (view === "panel" && e.key === "Escape" && !e.defaultPrevented) {
        invoke("hide_panel").catch(() => undefined);
      }
    };
    window.addEventListener("blur", unfocus);
    document.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("blur", unfocus);
      document.removeEventListener("keydown", onKey);
      window.removeEventListener("focus", shown);
      document.removeEventListener("pointerdown", touched, true);
      document.removeEventListener("keydown", touched, true);
      document.removeEventListener("focusin", focused);
    };
  }, [view]);

  return root;
}
