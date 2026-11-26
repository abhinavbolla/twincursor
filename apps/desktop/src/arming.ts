// How long Allow stays off after a control request appears. The Rust side enforces the same
// second (`ARMING` in calls.rs), counted from when the call bar could first be seen; this is
// only so the button doesn't look clickable before then.

/** How long Allow stays off after a control request appears. */
export const ALLOW_AFTER_MS = 1000;
