// Notices go away by themselves; errors stay until closed. Each notice line on screen has a
// clock: when it appeared, and whether the pointer is over it (which pauses it). Pure, so the
// timing is tested without timers; `useNoticeClock` (bits.tsx) runs it.

/** How long a notice stays on screen, not counting time with the pointer over it. */
export const NOTICE_MS = 6000;

export interface ShownLine {
  text: string;
  tone: "plain" | "bad";
}

export interface Clock {
  /** When it appeared, moved later by every pause. */
  shownAt: number;
  /** When the pointer went over it, while it is there. */
  pausedAt: number | null;
}

export type Clocks = ReadonlyMap<string, Clock>;

/** Clocks for the notices showing now: new ones start at `now`; lines no longer shown lose theirs. */
export function track(clocks: Clocks, showing: readonly ShownLine[], now: number): Map<string, Clock> {
  const next = new Map<string, Clock>();
  for (const line of showing) {
    if (line.tone === "plain") {
      next.set(line.text, clocks.get(line.text) ?? { shownAt: now, pausedAt: null });
    }
  }
  return next;
}

export function pause(clocks: Clocks, text: string, now: number): Map<string, Clock> {
  const next = new Map(clocks);
  const clock = clocks.get(text);
  if (clock !== undefined && clock.pausedAt === null) {
    next.set(text, { ...clock, pausedAt: now });
  }
  return next;
}

/** The pointer left: the time it spent over the line doesn't count. */
export function resume(clocks: Clocks, text: string, now: number): Map<string, Clock> {
  const next = new Map(clocks);
  const clock = clocks.get(text);
  if (clock !== undefined && clock.pausedAt !== null) {
    next.set(text, { shownAt: clock.shownAt + (now - clock.pausedAt), pausedAt: null });
  }
  return next;
}

/** The notices whose time is up. */
export function due(clocks: Clocks, now: number): string[] {
  return [...clocks].filter(([, c]) => c.pausedAt === null && now - c.shownAt >= NOTICE_MS).map(([text]) => text);
}

/** How long until the next notice is due, or null when none is counting down. */
export function nextDue(clocks: Clocks, now: number): number | null {
  const left = [...clocks.values()].filter((c) => c.pausedAt === null).map((c) => Math.max(0, c.shownAt + NOTICE_MS - now));
  return left.length === 0 ? null : Math.min(...left);
}

/** Closed lines that are no longer sent are forgotten, so the same words can show again later. */
export function forget(dismissed: ReadonlySet<string>, present: readonly (string | null)[]): ReadonlySet<string> {
  const kept = [...dismissed].filter((t) => present.includes(t));
  return kept.length === dismissed.size ? dismissed : new Set(kept);
}
