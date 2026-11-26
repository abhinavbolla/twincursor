// What the call bar shows for a home view and a call: one line that says what matters most right
// now, the buttons beside it, and the sections that open under it. A pure function, so every
// state is tested without rendering; CallBar.tsx only draws this.

import { safeAvatar } from "./accountState";
import { readouts, statsLine, type CallState, type Readout } from "./call";
import type { HomeView, Person } from "./home";
import type { Participant } from "./ipc";
import { joinNames, shortName, type Phrase } from "./people";
import type { FaceView } from "./panelModel";

/** At most this many faces in the row; the rest are "+N". */
export const MAX_FACES = 3;
/** At most this many notice lines under the row. */
export const MAX_NOTICES = 2;

export const REVOKE_KEYS = ["⌃", "⌥", "⌘", "R"];
export const MUTE_KEYS = ["⌃", "⌥", "⌘", "M"];
export const VIEWER_CONTROL_KEYS = ["F9"];

export type RowOp = "copy_link" | "take_back" | "control" | "never_mind" | "give_back";

export interface RowButton {
  op: RowOp;
  label: string;
  tone: "quiet" | "danger";
}

export interface MuteView {
  state: "on" | "muted" | "off";
  /** Its tooltip and accessible name. */
  label: string;
}

export interface LeaveView {
  label: "Stop" | "Leave" | "Cancel";
  op: "end" | "cancel_knock";
  enabled: boolean;
  /** Stop and Leave are the quiet-danger text button; Cancel is plain. */
  danger: boolean;
}

export interface KnockCard extends FaceView {
  userId: string;
  handle: string | null;
  /** Let in and Let in with control can be clicked. */
  armed: boolean;
}

export interface AskCard extends FaceView {
  request: number;
}

export interface NoticeLine {
  text: string;
  tone: "plain" | "bad";
}

export interface GuestRow extends FaceView {
  identity: string;
  state: "give" | "trusted" | "controlling";
}

export interface MenuView {
  link: string | null;
  /** The live numbers, or null before there are any (then `numbersEmpty` says so). */
  numbers: Readout[] | null;
  numbersEmpty: string;
  /** Sharer: one row per guest, to give control. */
  guests: GuestRow[];
  voice: string;
  /** The hotkey that mutes from any app (sharer only). */
  muteKeys: string[] | null;
}

export type BarModel =
  | { kind: "none" }
  | {
      kind: "call";
      role: "sharer" | "guest";
      dot: "good" | "pending" | "off";
      dotLabel: string;
      faces: FaceView[];
      /** People in the call beyond the faces shown. */
      extra: number;
      line: Phrase;
      /** Someone is controlling the sharer's Mac: the row's one loud state. */
      live: boolean;
      keys: string[] | null;
      button: RowButton | null;
      /** Null while there's no call to talk in yet (knocking, ending). */
      mute: MuteView | null;
      hasMenu: boolean;
      leave: LeaveView;
      knocks: KnockCard[];
      ask: AskCard | null;
      needsAccessibility: boolean;
      notices: NoticeLine[];
      menu: MenuView;
    };

export interface BarLocal {
  dismissed: ReadonlySet<string>;
  /** A flow the bar started was refused, with this sentence. */
  error: string | null;
}

export const NO_BAR_LOCAL: BarLocal = { dismissed: new Set(), error: null };

/** A photo for someone in the call: from their account, matched by engine identity. */
function faceOf(p: Participant, home: HomeView, host: Person | null): FaceView {
  const member = home.members.find((m) => `guest-${m.user_id}` === p.identity);
  const url = member?.avatar_url ?? (p.role === "sharer" ? (host?.avatar_url ?? null) : null);
  return { name: p.name, avatar: safeAvatar(url) };
}

function muteOf(call: CallState): MuteView {
  const voice = call.voice;
  if (voice === null) {
    return { state: "off", label: "Voice is connecting" };
  }
  if (!voice.sending) {
    return { state: "off", label: "Voice is off in this call" };
  }
  return voice.muted ? { state: "muted", label: "Unmute" } : { state: "on", label: "Mute" };
}

function voiceLine(call: CallState): string {
  const voice = call.voice;
  if (voice === null) {
    return "Voice is connecting…";
  }
  if (!voice.sending) {
    return "Voice is off in this call";
  }
  return `Your mic: ${voice.mic ?? "system default"}${voice.muted ? " (muted)" : ""}`;
}

function noticesOf(home: HomeView, call: CallState, local: BarLocal): NoticeLine[] {
  const lines: NoticeLine[] = [];
  const add = (text: string | null, tone: NoticeLine["tone"]) => {
    if (text !== null && !local.dismissed.has(text) && !lines.some((l) => l.text === text)) {
      lines.push({ text, tone });
    }
  };
  add(local.error, "bad");
  add(call.error, "bad");
  add(home.error, "bad");
  add(call.notice, "plain");
  add(home.notice, "plain");
  return lines.slice(0, MAX_NOTICES);
}

const DOT_LABEL = { good: "Connected", pending: "Reconnecting", off: "Not connected" } as const;

export function barModel(home: HomeView | null, call: CallState, local: BarLocal = NO_BAR_LOCAL): BarModel {
  if (home === null) {
    return { kind: "none" };
  }
  const stage = home.stage;
  if (stage.kind === "idle" || stage.kind === "starting") {
    return { kind: "none" };
  }
  const sharer = stage.kind === "hosting";
  const host = stage.kind === "knocking" || stage.kind === "joined" ? stage.host : null;
  const connection = call.connection;
  const dot: "good" | "pending" | "off" =
    stage.kind === "knocking" || stage.kind === "ending"
      ? "off"
      : connection === "connected"
        ? "good"
        : connection === "disconnected"
          ? "off"
          : "pending";

  const guests = call.participants.filter((p) => p.role !== "sharer");
  const control = call.control;
  const controller = control.state === "controlling" ? control.by : null;
  const live = sharer && controller !== null;

  // Faces: the people in the call, sharer first.
  const people: FaceView[] =
    stage.kind === "knocking" && host !== null
      ? [{ name: host.name, avatar: safeAvatar(host.avatar_url) }]
      : [...call.participants.filter((p) => p.role === "sharer"), ...guests].map((p) => faceOf(p, home, host));
  const inControl = control.state === "controlling";
  const faces = inControl ? [] : people.slice(0, MAX_FACES);
  const extra = inControl ? 0 : Math.max(0, people.length - MAX_FACES);

  let line: Phrase;
  let keys: string[] | null = null;
  let button: RowButton | null = null;

  if (stage.kind === "ending") {
    line = ["Ending the call…"];
  } else if (stage.kind === "knocking") {
    line = ["Waiting for ", { name: shortName(stage.host.name) }, " to let you in…"];
  } else if (live && controller !== null) {
    line = [{ name: shortName(controller.name) }, " is controlling"];
    keys = REVOKE_KEYS;
    button = { op: "take_back", label: "Take back", tone: "danger" };
  } else if (connection === "reconnecting") {
    line = ["Reconnecting…"];
  } else if (connection !== "connected") {
    line = ["Connecting…"];
  } else if (sharer) {
    const trusted = control.trusted.filter((t) => guests.some((g) => g.identity === t.identity));
    if (trusted.length > 0) {
      line = [{ name: joinNames(trusted.map((t) => shortName(t.name))) }, " can take control"];
      button = { op: "take_back", label: "Take back", tone: "quiet" };
    } else if (guests.length > 0) {
      line = ["With ", { name: joinNames(guests.map((g) => shortName(g.name))) }];
    } else {
      line = ["Sharing your screen"];
      button = { op: "copy_link", label: "Copy link", tone: "quiet" };
    }
  } else {
    const who = shortName(host?.name ?? "the sharer");
    if (control.state === "controlling") {
      line = ["Controlling ", { name: who }, "'s Mac"];
      keys = VIEWER_CONTROL_KEYS;
      button = { op: "give_back", label: "Give back", tone: "quiet" };
    } else if (control.state === "asked") {
      line = ["Asked ", { name: who }, " for control…"];
      button = { op: "never_mind", label: "Never mind", tone: "quiet" };
    } else {
      line = [{ name: who }, "'s screen"];
      button = { op: "control", label: "Control", tone: "quiet" };
    }
  }

  const inCall = stage.kind === "hosting" || stage.kind === "joined";
  const leave: LeaveView =
    stage.kind === "knocking"
      ? { label: "Cancel", op: "cancel_knock", enabled: true, danger: false }
      : { label: sharer ? "Stop" : "Leave", op: "end", enabled: stage.kind !== "ending", danger: true };

  return {
    kind: "call",
    role: sharer ? "sharer" : "guest",
    dot,
    dotLabel: DOT_LABEL[dot],
    faces,
    extra,
    line,
    live,
    keys,
    button,
    mute: inCall ? muteOf(call) : null,
    // While anyone is in control the row is about that alone: no faces and no menu, so the
    // line, the hotkey and Take back (or Give back) fit.
    hasMenu: inCall && !inControl,
    leave,
    knocks: sharer
      ? home.knocks.map((k) => ({
          userId: k.user_id,
          name: k.name,
          handle: k.handle,
          avatar: safeAvatar(k.avatar_url),
          armed: k.armed,
        }))
      : [],
    ask: (() => {
      const asked = call.asked;
      if (!sharer || asked === null) {
        return null;
      }
      const who = call.participants.find((p) => p.identity === asked.identity);
      const face = who === undefined ? { name: asked.name, avatar: null } : faceOf(who, home, host);
      return { request: asked.request, name: asked.name, avatar: face.avatar };
    })(),
    needsAccessibility: sharer && home.needs === "accessibility",
    notices: noticesOf(home, call, local),
    menu: {
      link: stage.kind === "hosting" ? stage.link : null,
      numbers: call.stats === null ? null : readouts(call.stats, call.role),
      numbersEmpty: statsLine(null, call.role),
      guests: sharer
        ? guests.map((g) => ({
            ...faceOf(g, home, host),
            identity: g.identity,
            state:
              controller?.identity === g.identity
                ? "controlling"
                : control.trusted.some((t) => t.identity === g.identity)
                  ? "trusted"
                  : "give",
          }))
        : [],
      voice: voiceLine(call),
      muteKeys: sharer ? MUTE_KEYS : null,
    },
  };
}
