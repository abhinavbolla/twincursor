// The call bar's row is a fixed 440 points. Its buttons keep their size and the line's text is
// what gives way, but a first name must stay readable. When it would be cut below about six
// characters, the row first drops the faces, then the keycaps. Pure, so the order is tested;
// CallBar measures and applies it.

/** About six characters of a name at 13 px, weight 500. */
export const MIN_NAME_PX = 48;

/** 0: everything; 1: no faces; 2: no faces and no keycaps. */
export type Compaction = 0 | 1 | 2;

/**
 * The next step for a row whose name is `shown` points wide out of the `full` it needs, given
 * whether the row has faces and keycaps to give up.
 */
export function compact(level: Compaction, shown: number, full: number, has: { faces: boolean; keys: boolean }): Compaction {
  if (shown >= Math.min(full, MIN_NAME_PX)) {
    return level;
  }
  if (level === 0 && has.faces) {
    return 1;
  }
  if (level < 2 && has.keys) {
    return 2;
  }
  return level;
}
