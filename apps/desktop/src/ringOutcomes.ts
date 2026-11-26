// What happened to a ring, for the People list: a person the host rang shows "No answer" or
// "Declined" for a while after the home view first says so, then their Call button comes back.
// Each outcome has a clock, started when it is first seen. Pure, so the timing is tested without
// timers; `useRingOutcomes` (bits.tsx) runs it.

import type { MemberState } from "./home";

/** How long "No answer" or "Declined" shows. */
export const RING_OUTCOME_MS = 10_000;

export type RingOutcome = "missed" | "declined";

export const RING_OUTCOME_LABEL: Record<RingOutcome, string> = {
  missed: "No answer",
  declined: "Declined",
};

export interface OutcomeClock {
  outcome: RingOutcome;
  /** When this outcome was first seen. */
  seenAt: number;
}

/** By user id. */
export type OutcomeClocks = ReadonlyMap<string, OutcomeClock>;

const isOutcome = (state: MemberState): state is RingOutcome => state === "missed" || state === "declined";

/**
 * Clocks for the members whose ring was missed or declined: one first seen (or changed) starts at
 * `now`, one still the same keeps its clock even after it has run out, and anyone else loses
 * theirs, so a ring missed again later shows again.
 */
export function trackOutcomes(
  clocks: OutcomeClocks,
  members: readonly { user_id: string; state: MemberState }[],
  now: number,
): Map<string, OutcomeClock> {
  const next = new Map<string, OutcomeClock>();
  for (const m of members) {
    if (isOutcome(m.state)) {
      const had = clocks.get(m.user_id);
      next.set(m.user_id, had !== undefined && had.outcome === m.state ? had : { outcome: m.state, seenAt: now });
    }
  }
  return next;
}

/** The outcomes to show at `now`, by user id. */
export function showingOutcomes(clocks: OutcomeClocks, now: number): Map<string, RingOutcome> {
  const showing = new Map<string, RingOutcome>();
  for (const [id, c] of clocks) {
    if (now - c.seenAt < RING_OUTCOME_MS) {
      showing.set(id, c.outcome);
    }
  }
  return showing;
}

/** How long until the next outcome stops showing, or null when none is showing. */
export function nextOutcomeDue(clocks: OutcomeClocks, now: number): number | null {
  const left = [...clocks.values()].map((c) => c.seenAt + RING_OUTCOME_MS - now).filter((ms) => ms > 0);
  return left.length === 0 ? null : Math.min(...left);
}
