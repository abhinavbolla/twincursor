import { useState } from "react";

import { copyText } from "./clipboard";
import type { Plan, Quality } from "./ipc";
import {
  blockers,
  describeAccess,
  isOk,
  needs,
  PERMISSION_LABELS,
  type PermissionName,
  type PermissionStatus,
} from "./permissions";
import { makeRoomCode, normalizeRoomCode } from "./room";
import { remember, remembered } from "./storage";

const QUALITIES: { value: Quality; label: string }[] = [
  { value: "1080p", label: "1080p" },
  { value: "1440p", label: "1440p" },
  { value: "4k", label: "4K" },
];

const asQuality = (v: string): Quality => (v === "1440p" || v === "4k" ? v : "1080p");

interface Props {
  endMessage: string | null;
  dismiss: () => void;
  start: (plan: Plan) => void;
  busy: boolean;
  permissions: PermissionStatus | null;
  permissionsError: string | null;
  ask: () => void;
  openPane: (name: PermissionName) => void;
}

export function StartScreen(props: Props) {
  const [name, setName] = useState(() => remembered("name", ""));
  const [room, setRoom] = useState("");
  const [quality, setQuality] = useState<Quality>(() => asQuality(remembered("quality", "1080p")));
  const [voice, setVoice] = useState(() => remembered("voice", "on") === "on");
  const [problem, setProblem] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);

  const go = (role: Plan["role"]) => {
    const trimmed = name.trim();
    if (trimmed === "") {
      setProblem("Type your name first. The other person sees it.");
      return;
    }
    let code = normalizeRoomCode(room);
    if (code === null && role === "share" && room.trim() === "") {
      code = makeRoomCode();
      setRoom(code);
    }
    if (code === null) {
      setProblem(
        room.trim() === ""
          ? "Paste the room code the sharer sent you."
          : "That isn't a room code. It should be 16 letters and digits, like 7kq2-m9xd-p4tz-hc3w.",
      );
      return;
    }
    const blocked = blockers(role, props.permissions);
    if (blocked.length > 0) {
      setProblem(
        `Sharing needs ${blocked.map((b) => PERMISSION_LABELS[b]).join(" and ")}. Allow it below first.`,
      );
      return;
    }
    setProblem(null);
    setRoom(code);
    remember("name", trimmed);
    remember("quality", quality);
    remember("voice", voice ? "on" : "off");
    props.start({ role, room: code, name: trimmed, quality, voice });
  };

  const make = () => {
    setRoom(makeRoomCode());
    setCopied(false);
  };

  const copy = async () => {
    const code = normalizeRoomCode(room);
    if (code !== null) {
      setCopied(await copyText(code));
    }
  };

  return (
    <>
      {props.endMessage !== null && (
        <p className="banner" role="status">
          {props.endMessage}{" "}
          <button type="button" className="link" onClick={props.dismiss}>
            OK
          </button>
        </p>
      )}

      <section className="card">
        <label className="field">
          <span>Your name</span>
          <input
            value={name}
            maxLength={64}
            autoComplete="off"
            placeholder="What the other person sees"
            onChange={(e) => setName(e.target.value)}
          />
        </label>

        <div className="field">
          <label htmlFor="room">Room code</label>
          <div className="row">
            <input
              id="room"
              className="code"
              value={room}
              spellCheck={false}
              autoComplete="off"
              placeholder="Paste one, or make one to share"
              onChange={(e) => {
                setRoom(e.target.value);
                setCopied(false);
              }}
            />
            <button type="button" className="secondary" onClick={make}>
              Make one
            </button>
            <button
              type="button"
              className="secondary"
              disabled={normalizeRoomCode(room) === null}
              onClick={() => void copy()}
            >
              {copied ? "Copied" : "Copy"}
            </button>
          </div>
          <p className="hint">
            Anyone with the app and this code can join, so share it only with the person you're
            calling.
          </p>
        </div>

        <fieldset className="field">
          <legend>Quality when you share</legend>
          <div className="choices">
            {QUALITIES.map((q) => (
              <label key={q.value} className="choice">
                <input
                  type="radio"
                  name="quality"
                  value={q.value}
                  checked={quality === q.value}
                  onChange={() => setQuality(q.value)}
                />
                <span>{q.label}</span>
              </label>
            ))}
          </div>
        </fieldset>

        <label className="choice">
          <input type="checkbox" checked={voice} onChange={(e) => setVoice(e.target.checked)} />
          <span>
            Voice{" "}
            <span className="muted">
              {voice
                ? "Your mic is on from the start, and you hear the other side."
                : "No mic and no sound in this call."}
            </span>
          </span>
        </label>

        {problem !== null && (
          <p className="bad" role="alert">
            {problem}
          </p>
        )}

        <div className="row actions">
          <button type="button" disabled={props.busy} onClick={() => go("share")}>
            Share my screen
          </button>
          <button type="button" disabled={props.busy} onClick={() => go("join")}>
            Join
          </button>
        </div>
      </section>

      <Permissions
        status={props.permissions}
        error={props.permissionsError}
        voice={voice}
        ask={props.ask}
        openPane={props.openPane}
      />
    </>
  );
}

function Permissions(props: {
  status: PermissionStatus | null;
  error: string | null;
  voice: boolean;
  ask: () => void;
  openPane: (name: PermissionName) => void;
}) {
  const need = needs(props.voice);
  const rows: PermissionName[] = ["screen_recording", "accessibility", "microphone"];
  const usedBy = (name: PermissionName): string => {
    const why = need.share.find((n) => n.name === name)?.why;
    if (why !== undefined) {
      return why;
    }
    return name === "microphone" ? "Only with voice on" : "Not needed";
  };
  return (
    <section className="card">
      <h2>Permissions</h2>
      <p className="hint">
        Only sharing needs Screen Recording and Accessibility.
      </p>
      {props.error !== null && <p className="bad">Couldn't read them: {props.error}</p>}
      <ul className="permissions">
        {rows.map((name) => {
          const access = props.status?.[name] ?? null;
          const ok = access !== null && isOk(access);
          return (
            <li key={name}>
              <span className={`dot ${access === null ? "" : ok ? "on" : "off"}`} aria-hidden="true" />
              <span className="perm-name">{PERMISSION_LABELS[name]}</span>
              <span className="perm-state">{access === null ? "Checking…" : describeAccess(access)}</span>
              <span className="perm-why muted">{usedBy(name)}</span>
              <span className="perm-buttons">
                {!ok && (
                  <button type="button" className="small secondary" onClick={props.ask}>
                    Ask
                  </button>
                )}
                <button type="button" className="small secondary" onClick={() => props.openPane(name)}>
                  Open Settings
                </button>
              </span>
            </li>
          );
        })}
      </ul>
    </section>
  );
}
