// The call bar: the only thing on screen during a call. A 440 × 48 pill that grows downward for
// a knock, a control request, a notice or the menu. What it shows comes from `barModel`; this
// file only draws it and runs the commands.

import { useCallback, useEffect, useId, useRef, useState } from "react";

import { Avatar, Faces, Keys, NoticeRow, PhraseLine } from "./bits";
import type { Bridge } from "./bridge";
import { barModel, type AskCard, type KnockCard, type MenuView, type RowOp } from "./barModel";
import { numbersReport, type CallState } from "./call";
import { ALLOW_AFTER_MS } from "./arming";
import { copyText } from "./clipboard";
import { allowControl, denyControl, mute, releaseControl, requestControl, revokeControl, unmute } from "./commands";
import type { HomeView } from "./home";
import { MicIcon, MicOffIcon, MoreIcon } from "./icons";
import type { EngineCommand } from "./ipc";
import { COPIED_MS } from "./Panel";
import { useCall, useHome } from "./useSurface";

/** How long a knock's Let in stays off (the Rust side's arming, shown as the progress line). */
export const KNOCK_ARMING_MS = 1000;

export function CallBarSurface({ bridge, menuOpen = false }: { bridge: Bridge; menuOpen?: boolean }) {
  const home = useHome(bridge);
  const call = useCall(bridge);
  return <CallBar bridge={bridge} home={home} call={call} menuOpen={menuOpen} />;
}

interface Props {
  bridge: Bridge;
  home: HomeView | null;
  call: CallState;
  menuOpen: boolean;
}

const sentence = (e: unknown): string => (e instanceof Error ? e.message : String(e));

/** "Copied" for a couple of seconds after copying, per thing copied. */
function useCopied(): [string | null, (what: string, text: string) => void] {
  const [copied, setCopied] = useState<string | null>(null);
  const timer = useRef<number | null>(null);
  useEffect(() => () => {
    if (timer.current !== null) {
      window.clearTimeout(timer.current);
    }
  }, []);
  const copy = useCallback((what: string, text: string) => {
    void copyText(text).then((ok) => {
      if (!ok) {
        return;
      }
      setCopied(what);
      if (timer.current !== null) {
        window.clearTimeout(timer.current);
      }
      timer.current = window.setTimeout(() => setCopied(null), COPIED_MS);
    });
  }, []);
  return [copied, copy];
}

export function CallBar({ bridge, home, call, menuOpen }: Props) {
  const [menu, setMenu] = useState(menuOpen);
  const [dismissed, setDismissed] = useState<ReadonlySet<string>>(new Set());
  const [error, setError] = useState<string | null>(null);
  const [answered, setAnswered] = useState<number | null>(null);
  const [copied, copy] = useCopied();
  const menuId = useId();
  const moreButton = useRef<HTMLButtonElement>(null);

  const model = barModel(home, call, { dismissed, error });

  useEffect(() => {
    if (!menu) {
      return;
    }
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        setMenu(false);
        moreButton.current?.focus();
      }
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [menu]);

  const send = (command: EngineCommand) => {
    bridge.send(command).catch((e: unknown) => setError(sentence(e)));
  };
  const run = (flow: () => Promise<unknown>) => {
    setError(null);
    flow().catch((e: unknown) => setError(sentence(e)));
  };
  const dismiss = (text: string) => {
    setDismissed((d) => new Set([...d, text]));
    if (text === error) {
      setError(null);
    }
  };

  if (model.kind === "none") {
    return (
      <div className="surface bar">
        <div className="bar-row" data-tauri-drag-region="deep">
          <span className="dot off" aria-hidden="true" />
          <span className="bar-line muted">Not in a call</span>
        </div>
      </div>
    );
  }

  const link = model.menu.link;
  const rowOp = (op: RowOp) => {
    switch (op) {
      case "copy_link":
        if (link !== null) {
          copy("link", link);
        }
        return;
      case "take_back":
        send(revokeControl());
        return;
      case "control":
        send(requestControl());
        return;
      case "never_mind":
      case "give_back":
        send(releaseControl());
        return;
    }
  };

  const ask = model.ask !== null && model.ask.request !== answered ? model.ask : null;
  const button = model.button;
  const leave = model.leave;

  return (
    <div className={`surface bar${model.live ? " live" : ""}`}>
      <div className="bar-row" data-tauri-drag-region="deep">
        <span className={`dot ${model.dot}`} role="img" aria-label={model.dotLabel} title={model.dotLabel} />
        <Faces faces={model.faces} extra={model.extra} />
        <PhraseLine phrase={model.line} className="bar-line" />
        {model.keys !== null && (
          <Keys keys={model.keys} label={model.role === "sharer" ? "Control Option Command R takes control back from any app" : "F9 in the viewer gives control back"} />
        )}
        <span className="bar-actions">
          {button !== null && (
            <button
              type="button"
              className={`small ${button.tone === "danger" ? "danger" : "quiet"}`}
              onClick={() => rowOp(button.op)}
            >
              {button.op === "copy_link" && copied === "link" ? "Copied" : button.label}
            </button>
          )}
          {(model.mute !== null || model.hasMenu) && (
            <span className="bar-icons">
              {model.mute !== null && (
                <button
                  type="button"
                  className={`icon-button${model.mute.state === "muted" ? " muted" : ""}`}
                  aria-label={model.mute.label}
                  title={model.mute.label}
                  aria-pressed={model.mute.state === "muted"}
                  disabled={model.mute.state === "off"}
                  onClick={() => send(model.mute?.state === "muted" ? unmute() : mute())}
                >
                  {model.mute.state === "on" ? <MicIcon /> : <MicOffIcon />}
                </button>
              )}
              {model.hasMenu && (
                <button
                  ref={moreButton}
                  type="button"
                  className="icon-button"
                  aria-label="More"
                  title="More"
                  aria-expanded={menu}
                  aria-controls={menuId}
                  onClick={() => setMenu((m) => !m)}
                >
                  <MoreIcon />
                </button>
              )}
            </span>
          )}
          <span className="bar-divider" aria-hidden="true" />
          <button
            type="button"
            className={`small ${leave.danger ? "quiet-danger" : "quiet"}`}
            disabled={!leave.enabled}
            onClick={() => run(() => (leave.op === "end" ? bridge.end() : bridge.cancelKnock()))}
          >
            {leave.label}
          </button>
        </span>
      </div>

      {model.knocks.map((k) => (
        <Knock key={k.userId} knock={k} answer={(letIn, control) => run(() => bridge.answerKnock(k.userId, letIn, control))} />
      ))}

      {ask !== null && (
        <Ask
          key={ask.request}
          ask={ask}
          answer={(allow) => {
            send(allow ? allowControl(ask.request) : denyControl(ask.request));
            setAnswered(ask.request);
          }}
        />
      )}

      {model.needsAccessibility && (
        <div className="bar-section bar-slim" role="status">
          <span className="bar-slim-text">TwinCursor needs Accessibility to give control.</span>
          <button type="button" className="small" onClick={() => void bridge.openPane("accessibility")}>
            Open Settings
          </button>
        </div>
      )}

      {model.notices.length > 0 && (
        <div className="bar-section bar-notices">
          {model.notices.map((n) => (
            <NoticeRow key={n.text} text={n.text} tone={n.tone} close={() => dismiss(n.text)} />
          ))}
        </div>
      )}

      {menu && model.hasMenu && (
        <BarMenu
          id={menuId}
          menu={model.menu}
          copied={copied}
          copyLink={() => link !== null && copy("link", link)}
          copyNumbers={() => copy("numbers", numbersReport(call, new Date()))}
          giveControl={(identity) => run(() => bridge.giveControl(identity))}
        />
      )}
    </div>
  );
}

/** The thin line that runs across a card's top while its buttons are still off. */
function Arming({ ms }: { ms: number }) {
  return <span className="arming" style={{ animationDuration: `${ms}ms` }} aria-hidden="true" />;
}

function Knock({ knock, answer }: { knock: KnockCard; answer: (letIn: boolean, control: boolean) => void }) {
  const id = useId();
  return (
    <div className="bar-section bar-card" role="alertdialog" aria-labelledby={`${id}-t`} aria-describedby={`${id}-d`}>
      {!knock.armed && <Arming ms={KNOCK_ARMING_MS} />}
      <div className="bar-card-who">
        <Avatar face={knock} size={36} />
        <div className="bar-card-names">
          <p id={`${id}-t`} className="bar-card-title">
            <PhraseLine phrase={[{ name: knock.name }, " wants to join"]} />
          </p>
          <p id={`${id}-d`} className="bar-card-meta">
            {knock.handle !== null ? `@${knock.handle}` : "No GitHub handle"}
          </p>
        </div>
      </div>
      <div className="bar-card-actions">
        <button type="button" className="small quiet" onClick={() => answer(false, false)}>
          Turn away
        </button>
        <span className="bar-fill" />
        <button type="button" className="small" disabled={!knock.armed} onClick={() => answer(true, true)}>
          Let in with control
        </button>
        <button type="button" className="small primary" disabled={!knock.armed} onClick={() => answer(true, false)}>
          Let in
        </button>
      </div>
    </div>
  );
}

function Ask({ ask, answer }: { ask: AskCard; answer: (allow: boolean) => void }) {
  const id = useId();
  const [armed, setArmed] = useState(false);
  useEffect(() => {
    const timer = window.setTimeout(() => setArmed(true), ALLOW_AFTER_MS);
    return () => window.clearTimeout(timer);
  }, []);
  return (
    <div className="bar-section bar-card" role="alertdialog" aria-labelledby={`${id}-t`} aria-describedby={`${id}-d`}>
      {!armed && <Arming ms={ALLOW_AFTER_MS} />}
      <div className="bar-card-who">
        <Avatar face={ask} size={36} />
        <div className="bar-card-names">
          <p id={`${id}-t`} className="bar-card-title">
            <PhraseLine phrase={[{ name: ask.name }, " wants to control your Mac"]} />
          </p>
          <p id={`${id}-d`} className="bar-card-meta wraps">
            They'll use your mouse and keyboard until you take control back
          </p>
        </div>
      </div>
      <div className="bar-card-actions">
        <span className="bar-fill" />
        <button type="button" className="small quiet" onClick={() => answer(false)}>
          Deny
        </button>
        <button type="button" className="small primary" disabled={!armed} onClick={() => answer(true)}>
          Allow
        </button>
      </div>
    </div>
  );
}

function BarMenu(props: {
  id: string;
  menu: MenuView;
  copied: string | null;
  copyLink: () => void;
  copyNumbers: () => void;
  giveControl: (identity: string) => void;
}) {
  const { menu } = props;
  return (
    <div className="bar-section bar-menu" id={props.id} role="group" aria-label="Call details">
      {menu.link !== null && (
        <div className="bar-menu-row">
          <code className="link-text" title={menu.link}>
            {menu.link}
          </code>
          <button type="button" className="small quiet" onClick={props.copyLink}>
            {props.copied === "link" ? "Copied" : "Copy link"}
          </button>
        </div>
      )}

      <div className="bar-menu-group">
        <div className="bar-menu-row">
          <h2 className="bar-menu-label">Numbers</h2>
          <button type="button" className="small quiet" disabled={menu.numbers === null} onClick={props.copyNumbers}>
            {props.copied === "numbers" ? "Copied" : "Copy numbers"}
          </button>
        </div>
        {menu.numbers === null ? (
          <p className="bar-menu-empty">{menu.numbersEmpty}</p>
        ) : (
          <dl className="bar-readout">
            {menu.numbers.map((r) => (
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
      </div>

      {menu.guests.length > 0 && (
        <ul className="bar-menu-group bar-guests" aria-label="Control">
          {menu.guests.map((g) => (
            <li key={g.identity} className="bar-menu-row">
              <Avatar face={g} size={24} />
              <bdi className="bar-guest-name" title={g.name}>
                {g.name}
              </bdi>
              {g.state === "give" ? (
                <button type="button" className="small quiet" onClick={() => props.giveControl(g.identity)}>
                  Give control
                </button>
              ) : (
                <span className="bar-guest-state">{g.state === "controlling" ? "Controlling" : "Can take control"}</span>
              )}
            </li>
          ))}
        </ul>
      )}

      <div className="bar-menu-row bar-voice">
        <span className="bar-voice-text">{menu.voice}</span>
        {menu.muteKeys !== null && <Keys keys={menu.muteKeys} label="Control Option Command M mutes from any app" />}
      </div>
    </div>
  );
}
