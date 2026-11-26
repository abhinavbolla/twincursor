// What the panel (the home, opened from the menu bar icon) shows for a home view and a call:
// a pure function, so every state is tested without rendering. Panel.tsx only draws this.

import { safeAvatar } from "./accountState";
import type { CallState } from "./call";
import type { HomeView, MemberState } from "./home";
import { shortName, type Phrase } from "./people";
import { CANT_SHARE_LINE, MAC, type PlatformInfo } from "./platform";
import { RING_OUTCOME_LABEL, type RingOutcome } from "./ringOutcomes";

export interface FaceView {
  name: string;
  /** A photo the window may load, or null for initials. */
  avatar: string | null;
}

export interface MeView extends FaceView {
  handle: string | null;
}

export interface RingCard extends FaceView {
  callId: string;
}

export type PersonAction =
  /** Call: rings them (and shares first when you aren't sharing). */
  | { kind: "call"; enabled: boolean }
  /** Where they are in your call instead of a button. */
  | { kind: "status"; label: string }
  | { kind: "none" };

export interface PersonRow extends FaceView {
  id: string;
  handle: string | null;
  action: PersonAction;
}

export type CallOp = "end" | "cancel_knock";

export type PanelMain =
  /** Share my screen and Paste link. `primary` is false while a ring card holds the primary button. */
  | { kind: "share"; busy: boolean; primary: boolean }
  /** Where this app can't share (Windows): one line saying so, and Paste link as the action. */
  | { kind: "join"; note: string; busy: boolean; primary: boolean }
  | {
      kind: "call";
      dot: "good" | "pending" | "off";
      line: Phrase;
      action: { label: "Stop sharing" | "Cancel" | "Leave"; op: CallOp } | null;
      /** Hosting: the call link, for Copy link. */
      link: string | null;
    };

export type PanelModel =
  /** The first home view hasn't arrived yet. */
  | { kind: "loading" }
  /** This build names no account backend. */
  | { kind: "off" }
  | { kind: "signed_out"; waiting: boolean }
  | {
      kind: "home";
      me: MeView;
      rings: RingCard[];
      /** Screen Recording is missing: the notice above the main action. */
      needsScreenRecording: boolean;
      error: string | null;
      notice: string | null;
      main: PanelMain;
      people: PersonRow[];
      serverProblem: string | null;
    };

export interface PanelLocal {
  /** Lines the person closed, by their text. */
  dismissed: ReadonlySet<string>;
  /** A flow the panel started was refused, with this sentence. */
  error: string | null;
  /** A sign-in this panel started is waiting for the browser. */
  signingIn: boolean;
  /** Rings that just went unanswered or were declined, by user id, while they show. */
  outcomes: ReadonlyMap<string, RingOutcome>;
}

export const NO_LOCAL: PanelLocal = { dismissed: new Set(), error: null, signingIn: false, outcomes: new Map() };

const MEMBER_LABEL: Partial<Record<MemberState, string>> = {
  ringing: "Ringing…",
  knocking: "Knocking",
  admitted: "In the call",
};

const dotOf = (call: CallState): "good" | "pending" | "off" =>
  call.connection === "connected" ? "good" : call.connection === "disconnected" ? "off" : "pending";

function mainOf(home: HomeView, call: CallState, platform: PlatformInfo): PanelMain {
  const stage = home.stage;
  switch (stage.kind) {
    case "idle":
    case "starting":
      return platform.can_share
        ? { kind: "share", busy: stage.kind === "starting", primary: home.rings.length === 0 }
        : { kind: "join", note: CANT_SHARE_LINE, busy: stage.kind === "starting", primary: home.rings.length === 0 };
    case "hosting":
      return {
        kind: "call",
        dot: dotOf(call),
        line: ["You're sharing your screen"],
        action: { label: "Stop sharing", op: "end" },
        link: stage.link,
      };
    case "knocking":
      return {
        kind: "call",
        dot: "pending",
        line: ["Waiting for ", { name: shortName(stage.host.name) }, " to let you in…"],
        action: { label: "Cancel", op: "cancel_knock" },
        link: null,
      };
    case "joined":
      return {
        kind: "call",
        dot: dotOf(call),
        line: ["In ", { name: shortName(stage.host.name) }, "'s call"],
        action: { label: "Leave", op: "end" },
        link: null,
      };
    case "ending":
      return { kind: "call", dot: "off", line: ["Ending the call…"], action: null, link: null };
  }
}

function actionFor(home: HomeView, id: string, outcomes: ReadonlyMap<string, RingOutcome>, platform: PlatformInfo): PersonAction {
  if (!platform.can_share) {
    // Calling someone means sharing your screen first.
    return { kind: "none" };
  }
  switch (home.stage.kind) {
    case "idle":
      return { kind: "call", enabled: true };
    case "starting":
      return { kind: "call", enabled: false };
    case "hosting": {
      const member = home.members.find((m) => m.user_id === id);
      const label = member === undefined ? undefined : MEMBER_LABEL[member.state];
      if (label !== undefined) {
        return { kind: "status", label };
      }
      // A ring that just went unanswered or was declined says so for a while, then Call is back.
      const outcome = outcomes.get(id);
      return outcome !== undefined && member?.state === outcome
        ? { kind: "status", label: RING_OUTCOME_LABEL[outcome] }
        : { kind: "call", enabled: true };
    }
    case "knocking":
    case "joined":
    case "ending":
      // Ringing someone needs you to be the one sharing.
      return { kind: "none" };
  }
}

export function panelModel(
  home: HomeView | null,
  call: CallState,
  local: PanelLocal = NO_LOCAL,
  platform: PlatformInfo = MAC,
): PanelModel {
  if (home === null) {
    return { kind: "loading" };
  }
  const account = home.account;
  if (!account.configured) {
    return { kind: "off" };
  }
  if (!account.signed_in) {
    return { kind: "signed_out", waiting: account.signing_in || local.signingIn };
  }
  const shown = (text: string | null): string | null => (text === null || local.dismissed.has(text) ? null : text);
  const me = account.me;
  return {
    kind: "home",
    me: {
      name: me?.name ?? "Signed in",
      handle: me?.handle ?? null,
      avatar: safeAvatar(me?.avatar_url ?? null),
    },
    rings: home.rings.map((r) => ({ callId: r.call_id, name: r.host.name, avatar: safeAvatar(r.host.avatar_url) })),
    needsScreenRecording: platform.can_share && home.needs === "screen_recording",
    error: shown(local.error ?? home.error),
    notice: shown(home.notice),
    main: mainOf(home, call, platform),
    people: home.contacts.map((c) => ({
      id: c.id,
      name: c.name,
      handle: c.handle,
      avatar: safeAvatar(c.avatar_url),
      action: actionFor(home, c.id, local.outcomes, platform),
    })),
    serverProblem: home.server_problem,
  };
}
