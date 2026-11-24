import { useEffect, useState } from "react";

import { numbersReport, readouts, statsLine, unverified, type CallState } from "./call";
import { copyText } from "./clipboard";
import {
  allowControl,
  denyControl,
  mute,
  releaseControl,
  requestControl,
  revokeControl,
  unmute,
} from "./commands";
import type { EngineCommand } from "./ipc";

interface Props {
  call: CallState;
  send: (command: EngineCommand) => void;
  leave: () => void;
  answered: () => void;
  dismiss: () => void;
}

const CONNECTION: Record<string, string> = {
  connecting: "Connecting…",
  connected: "Connected",
  reconnecting: "Connection lost, reconnecting…",
  disconnected: "Disconnected",
};

/** A hotkey, one keycap per key. */
function Keys({ keys }: { keys: string[] }) {
  return (
    <span className="keys" aria-label={keys.join(" ")}>
      {keys.map((k) => (
        <kbd key={k}>{k}</kbd>
      ))}
    </span>
  );
}

export function CallScreen({ call, send, leave, answered, dismiss }: Props) {
  const sharer = call.role === "share";
  const others = call.participants;
  const host = others.find((p) => p.role === "sharer");
  const room = roomOf(call.args);

  return (
    <>
      <section className="card">
        <div className="row spread">
          <div className="stack">
            <h2>{sharer ? "Sharing your screen" : host ? `Watching ${host.name}'s screen` : "Joining"}</h2>
            <p className={`status ${call.connection ?? ""}`}>
              {CONNECTION[call.connection ?? "connecting"]}
              {room !== null && (
                <>
                  {" · room "}
                  <code>{room}</code>
                </>
              )}
            </p>
          </div>
          <button type="button" className="quiet-danger" onClick={leave}>
            Leave
          </button>
        </div>

        {call.notice !== null && (
          <p className="banner" role="status">
            {call.notice}
          </p>
        )}
        {call.error !== null && (
          <p className="banner bad" role="alert">
            {call.error}{" "}
            <button type="button" className="link" onClick={dismiss}>
              OK
            </button>
          </p>
        )}

        <div className="stack">
          <h3>In the call</h3>
          {others.length === 0 ? (
            <p className="muted">
              {sharer ? "Waiting for someone to join with the room code." : "Waiting for the sharer."}
            </p>
          ) : (
            <ul className="people">
              {others.map((p) => (
                <li key={p.identity}>
                  {unverified(p.name)} <span className="muted">· {p.role === "sharer" ? "sharing" : "guest"}</span>
                </li>
              ))}
            </ul>
          )}
        </div>
        <p className="hint">Names are what people typed. Make sure you know who you're talking to.</p>
      </section>

      {sharer ? <SharerControl call={call} send={send} answered={answered} /> : <GuestControl call={call} send={send} />}

      <section className="card">
        <div className="row spread">
          <h2>Voice</h2>
          {call.voice?.sending === true && (
            <button type="button" onClick={() => send(call.voice?.muted ? unmute() : mute())}>
              {call.voice.muted ? "Unmute" : "Mute"}
            </button>
          )}
        </div>
        <p>
          {call.voice === null
            ? "…"
            : !call.voice.sending
              ? "Voice is off in this call. No mic and no sound."
              : call.voice.muted
                ? "You're muted."
                : `Your mic is on${call.voice.mic ? ` (${call.voice.mic})` : ""}.`}
        </p>
        {call.voice?.sending === true && (
          <p className="hint">
            {sharer ? (
              <>
                <Keys keys={["⌃", "⌥", "⌘", "M"]} /> mutes from any app
              </>
            ) : (
              <>
                <Keys keys={["M"]} /> in the viewer mutes
              </>
            )}
          </p>
        )}
      </section>

      <Numbers call={call} />
    </>
  );
}

/**
 * How long Allow stays off after a request appears. The window comes to the front for a request,
 * so a click or key press already on its way to another app must not land on Allow.
 */
export const ALLOW_AFTER_MS = 1000;

function SharerControl({ call, send, answered }: Pick<Props, "call" | "send" | "answered">) {
  const asked = call.asked;
  const by = call.control.by;
  const request = asked?.request ?? null;
  const [armed, setArmed] = useState<number | null>(null);
  useEffect(() => {
    if (request === null) {
      return;
    }
    const timer = setTimeout(() => setArmed(request), ALLOW_AFTER_MS);
    return () => clearTimeout(timer);
  }, [request]);
  return (
    <section className="card">
      <h2>Control of your computer</h2>
      {asked !== null ? (
        <div className="prompt" role="alertdialog" aria-labelledby="ask-title">
          <p id="ask-title">
            <strong>{asked.name}</strong> wants to control your computer.
          </p>
          <p className="hint">
            That's the name they typed, not proof of who they are. Allow it only if it's the person
            you're talking to. They can use your mouse and keyboard until you take control back.
          </p>
          <div className="row">
            <button
              type="button"
              className="primary"
              disabled={armed !== asked.request}
              onClick={() => {
                send(allowControl(asked.request));
                answered();
              }}
            >
              Allow
            </button>
            <button
              type="button"
              onClick={() => {
                send(denyControl(asked.request));
                answered();
              }}
            >
              Deny
            </button>
          </div>
        </div>
      ) : call.control.state === "controlling" && by !== null ? (
        <div className="prompt live">
          <p>
            <strong>{unverified(by.name)}</strong> is controlling your computer.
          </p>
          <button type="button" className="danger" onClick={() => send(revokeControl())}>
            Take back control
          </button>
          <p className="hint">
            <Keys keys={["⌃", "⌥", "⌘", "R"]} /> takes control back too, from any app
          </p>
        </div>
      ) : (
        <p className="muted">
          Nobody is controlling your computer. A guest can ask, and you decide each time.
        </p>
      )}
    </section>
  );
}

function GuestControl({ call, send }: Pick<Props, "call" | "send">) {
  const host = call.participants.find((p) => p.role === "sharer");
  const who = host?.name ?? "the sharer";
  const state = call.control.state;
  return (
    <section className="card">
      <h2>Control</h2>
      {state === "controlling" ? (
        <>
          <p>You're controlling {who}'s computer. Click into the shared screen's window to use it.</p>
          <button type="button" onClick={() => send(releaseControl())}>
            Give back control
          </button>
          <p className="hint">
            <Keys keys={["F9"]} /> in the viewer gives it back too
          </p>
        </>
      ) : state === "asked" ? (
        <>
          <p>Waiting for {who} to allow it…</p>
          <button type="button" onClick={() => send(releaseControl())}>
            Never mind
          </button>
        </>
      ) : (
        <>
          <p className="muted">You're watching. You can ask {who} to let you use their mouse and keyboard.</p>
          <button type="button" className="primary" disabled={host === undefined} onClick={() => send(requestControl())}>
            Ask for control
          </button>
          <p className="hint">
            <Keys keys={["F9"]} /> in the viewer asks too
          </p>
        </>
      )}
    </section>
  );
}

function Numbers({ call }: { call: CallState }) {
  const [copied, setCopied] = useState(false);
  const line = statsLine(call.stats, call.role);
  return (
    <section className="card">
      <div className="row spread">
        <h2>Numbers</h2>
        <button
          type="button"
          className="small"
          onClick={() => void copyText(numbersReport(call, new Date())).then(setCopied)}
        >
          {copied ? "Copied" : "Copy numbers"}
        </button>
      </div>
      {call.stats === null ? (
        <p className="muted">{line}</p>
      ) : (
        <dl className="readout" aria-label={line}>
          {readouts(call.stats, call.role).map((r) => (
            <div key={r.label}>
              <dt>{r.label}</dt>
              <dd className="figure">
                {r.value}
                {r.unit !== null && <small>{r.unit}</small>}
              </dd>
            </div>
          ))}
        </dl>
      )}
    </section>
  );
}

/** The room, from the engine's flags. */
function roomOf(args: string[]): string | null {
  const flag = args.find((a) => a.startsWith("--room="));
  return flag === undefined ? null : flag.slice("--room=".length);
}
