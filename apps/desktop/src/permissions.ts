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
  /** Why, in a few words. */
  why: string;
}

/** What sharing needs, and what joining needs, for this voice choice. */
export function needs(voice: boolean): { share: Need[]; join: Need[] } {
  const mic: Need[] = voice ? [{ name: "microphone", why: "for voice" }] : [];
  return {
    share: [
      { name: "screen_recording", why: "to share your screen" },
      { name: "accessibility", why: "to let a guest control it" },
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
