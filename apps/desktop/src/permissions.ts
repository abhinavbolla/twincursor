// What macOS lets the engine do (`tc-engine permissions`), and what each kind of call needs.

export type Access =
  | "granted"
  | "not_granted"
  | "not_asked"
  | "denied"
  | "restricted"
  | "not_applicable";

export type PermissionName = "screen_recording" | "accessibility" | "microphone";

export interface PermissionStatus {
  screen_recording: Access;
  accessibility: Access;
  microphone: Access;
}

/** The Rust side's refusals for a missing permission (`calls.rs`). The needs section is where
 * that is said, so they aren't repeated as notices, and once it is granted they are stale. */
export const NEEDS_ACCESSIBILITY_ERROR =
  "TwinCursor needs Accessibility to give control. Turn it on in System Settings, then try again.";
export const NEEDS_SCREEN_RECORDING_ERROR =
  "TwinCursor needs Screen Recording to share your screen. Turn it on in System Settings, then try again.";

/** What the running engine says when it can't make real input (`NEEDS_ACCESSIBILITY` in
 * crates/engine/src/sharer.rs). */
export const ENGINE_NEEDS_ACCESSIBILITY = "TwinCursor needs Accessibility to let someone control this Mac.";

/** The engine refused although the app's fresh check said yes: the share's process can't use
 * the grant, and a new one can. */
export const RESHARE_FOR_ACCESSIBILITY =
  "macOS says Accessibility is on, but this share can't use it yet. Stop and share again for it to take effect.";

/** Still refused after macOS was asked and the person went to System Settings or tried again.
 * It doesn't claim to know why. */
export const STILL_NO = "macOS still says no. If TwinCursor is already on in the list, it may be the switch from an older copy.";
export const RESET_LABEL = "Reset and ask again";

/** A missing permission's refusal, rewritten for where it shows, or null to leave it out. */
export function permissionLine(text: string, needs: "screen_recording" | "accessibility" | null): string | null {
  if (text === NEEDS_ACCESSIBILITY_ERROR || text === NEEDS_SCREEN_RECORDING_ERROR) {
    return null;
  }
  if (text === ENGINE_NEEDS_ACCESSIBILITY) {
    return needs === "accessibility" ? null : RESHARE_FOR_ACCESSIBILITY;
  }
  return text;
}

export const PERMISSION_LABELS: Record<PermissionName, string> = {
  screen_recording: "Screen Recording",
  accessibility: "Accessibility",
  microphone: "Microphone",
};

const ACCESS: readonly Access[] = [
  "granted",
  "not_granted",
  "not_asked",
  "denied",
  "restricted",
  "not_applicable",
];

const isAccess = (v: unknown): v is Access => typeof v === "string" && (ACCESS as string[]).includes(v);

/** Reads the engine's report, or null if it isn't one. */
export function parsePermissions(value: unknown): PermissionStatus | null {
  if (typeof value !== "object" || value === null) {
    return null;
  }
  const v = value as Record<string, unknown>;
  const { screen_recording, accessibility, microphone } = v;
  if (!isAccess(screen_recording) || !isAccess(accessibility) || !isAccess(microphone)) {
    return null;
  }
  return { screen_recording, accessibility, microphone };
}

export const isOk = (a: Access): boolean => a === "granted" || a === "not_applicable";

export function describeAccess(a: Access): string {
  switch (a) {
    case "granted":
      return "Allowed";
    case "not_granted":
      return "Not allowed yet";
    case "not_asked":
      return "Not asked yet";
    case "denied":
      return "Turned off";
    case "restricted":
      return "Blocked by a profile";
    case "not_applicable":
      return "Not needed here";
  }
}

export interface Need {
  name: PermissionName;
  /** Why, in a few words, as the window shows it. */
  why: string;
}

/** What sharing needs, and what joining needs, for this voice choice. */
export function needs(voice: boolean): { share: Need[]; join: Need[] } {
  const mic: Need[] = voice ? [{ name: "microphone", why: "For voice" }] : [];
  return {
    share: [
      { name: "screen_recording", why: "To share your screen" },
      { name: "accessibility", why: "To let a guest control your Mac" },
      ...mic,
    ],
    join: mic,
  };
}

/**
 * What stops a call from starting: sharing can't work without Screen Recording. Everything else
 * only warns: a missing Accessibility grant only matters once control is allowed, and the
 * engine asks for the mic itself. An unknown status blocks nothing.
 */
export function blockers(role: "share" | "join", status: PermissionStatus | null): PermissionName[] {
  if (status === null || role === "join") {
    return [];
  }
  return isOk(status.screen_recording) ? [] : ["screen_recording"];
}
