// The account as the window sees it (`account_state`): never a token, only who is signed in.

export interface Me {
  id: string;
  name: string;
  handle: string | null;
  avatar_url: string | null;
}

export interface AccountState {
  /** This build names an account backend. Without one, nothing about accounts is shown. */
  configured: boolean;
  signed_in: boolean;
  me: Me | null;
  /** A sign-in is waiting for the browser. */
  signing_in: boolean;
  /** They may start calls (share, ring) on this server. The server decides; true until it says. */
  can_host: boolean;
  /** They asked whoever runs the server to let them start calls. */
  asked_to_host: boolean;
}

/** What the Account card shows. */
export type AccountView =
  | { kind: "hidden" }
  | { kind: "signed_out" }
  | { kind: "waiting" }
  | { kind: "signed_in"; name: string; handle: string | null; avatar: string | null };

/** The only host avatars are loaded from; the window's CSP allows it and nothing else. */
export const AVATAR_ORIGIN = "https://avatars.githubusercontent.com";

const isText = (v: unknown): v is string => typeof v === "string";
const isTextOrNull = (v: unknown): v is string | null => v === null || typeof v === "string";

function parseMe(value: unknown): Me | null {
  if (typeof value !== "object" || value === null) {
    return null;
  }
  const v = value as Record<string, unknown>;
  if (!isText(v.id) || !isText(v.name) || !isTextOrNull(v.handle) || !isTextOrNull(v.avatar_url)) {
    return null;
  }
  return { id: v.id, name: v.name, handle: v.handle, avatar_url: v.avatar_url };
}

/** Reads `account_state`'s answer, or null if it isn't one. */
export function parseAccountState(value: unknown): AccountState | null {
  if (typeof value !== "object" || value === null) {
    return null;
  }
  const v = value as Record<string, unknown>;
  if (
    typeof v.configured !== "boolean" ||
    typeof v.signed_in !== "boolean" ||
    typeof v.signing_in !== "boolean" ||
    typeof v.can_host !== "boolean" ||
    typeof v.asked_to_host !== "boolean"
  ) {
    return null;
  }
  const me = v.me === null || v.me === undefined ? null : parseMe(v.me);
  if (v.me !== null && v.me !== undefined && me === null) {
    return null;
  }
  return {
    configured: v.configured,
    signed_in: v.signed_in,
    me,
    signing_in: v.signing_in,
    can_host: v.can_host,
    asked_to_host: v.asked_to_host,
  };
}

/** An avatar URL the window may load: GitHub's avatar host over HTTPS, nothing else. */
export function safeAvatar(url: string | null): string | null {
  if (url === null) {
    return null;
  }
  try {
    const parsed = new URL(url);
    return parsed.origin === AVATAR_ORIGIN ? parsed.href : null;
  } catch {
    return null;
  }
}

/** What to show, from the state and whether this window is waiting on a sign-in it started. */
export function accountView(state: AccountState | null, waiting: boolean): AccountView {
  if (state === null || !state.configured) {
    return { kind: "hidden" };
  }
  if (state.signed_in) {
    const me = state.me;
    return {
      kind: "signed_in",
      name: me?.name ?? "Signed in",
      handle: me?.handle ?? null,
      avatar: safeAvatar(me?.avatar_url ?? null),
    };
  }
  return waiting || state.signing_in ? { kind: "waiting" } : { kind: "signed_out" };
}
