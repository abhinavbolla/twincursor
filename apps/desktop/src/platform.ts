// What the platform the app runs on can do, as the Rust side says (`platform_info`). Only a Mac
// shares its screen; on Windows the app is a guest: it joins calls by link or by answering a
// ring, watches, talks and takes control, but has no Share or Call.

export interface PlatformInfo {
  /** `macos`, `windows` or `linux`. */
  os: string;
  /** This app can share its screen, and so start a call or ring someone. */
  can_share: boolean;
}

export const MAC: PlatformInfo = { os: "macos", can_share: true };
export const WINDOWS: PlatformInfo = { os: "windows", can_share: false };

/** The Rust side's answer, checked; null when it can't be read. */
export function parsePlatform(value: unknown): PlatformInfo | null {
  if (typeof value !== "object" || value === null) {
    return null;
  }
  const v = value as Record<string, unknown>;
  return typeof v.os === "string" && typeof v.can_share === "boolean" ? { os: v.os, can_share: v.can_share } : null;
}

/** What `share` and `ring` answer where sharing isn't built (`platform::CANT_SHARE`). */
export const CANT_SHARE = "Sharing your screen from Windows isn't available yet. You can join a call with a link.";

/** The dev page's `&platform=windows` (or `macos`); null when it names none. */
export function platformOfParams(params: URLSearchParams): PlatformInfo | null {
  switch (params.get("platform")) {
    case "windows":
      return WINDOWS;
    case "macos":
      return MAC;
    default:
      return null;
  }
}

/** Where Share my screen would be, on a platform that can't share. */
export const CANT_SHARE_LINE = "Sharing from Windows isn't available yet. You can join a call with a link.";

/** The signed-out panel's line. */
export const signInLine = (platform: PlatformInfo): string =>
  platform.can_share ? "Sign in to share your screen and call people." : "Sign in to join calls with a link.";

/** The OS a page's user agent names, for styling before the Rust side has answered. */
export function osOfUserAgent(userAgent: string): "windows" | "macos" | "other" {
  if (/Windows NT/.test(userAgent)) {
    return "windows";
  }
  return /Mac OS X|Macintosh/.test(userAgent) ? "macos" : "other";
}
