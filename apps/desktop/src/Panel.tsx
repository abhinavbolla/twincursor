// The panel: the home, opened from the menu bar icon. 340 px wide; the people list scrolls
// inside it. What it shows comes from `panelModel`; this file only draws it and runs the flows.

import { useCallback, useEffect, useRef, useState } from "react";

import { Avatar, NoticeRow, PhraseLine, useNoticeClock, useRingOutcomes } from "./bits";
import type { Bridge } from "./bridge";
import type { CallState } from "./call";
import { copyText } from "./clipboard";
import type { HomeView } from "./home";
import { Mark, MoreIcon } from "./icons";
import { Menu, type MenuItem } from "./Menu";
import { forget, type ShownLine } from "./notices";
import { panelModel, type MeView, type PanelMain, type PersonRow, type RingCard } from "./panelModel";
import { MAC, signInLine, type PlatformInfo } from "./platform";
import { useCall, useHome, usePlatform } from "./useSurface";

/** How long "Copied" shows after a copy. */
export const COPIED_MS = 2000;

export function PanelSurface({ bridge, menuOpen = false }: { bridge: Bridge; menuOpen?: boolean }) {
  const home = useHome(bridge);
  const call = useCall(bridge);
  const platform = usePlatform(bridge);
  // Nothing is drawn until the platform is known, so a Windows panel never flashes Share.
  return <Panel bridge={bridge} home={platform === null ? null : home} call={call} menuOpen={menuOpen} platform={platform ?? MAC} />;
}

interface Props {
  bridge: Bridge;
  home: HomeView | null;
  call: CallState;
  /** Opens with the account menu open (the dev page's `out-menu` state). */
  menuOpen?: boolean;
  platform?: PlatformInfo;
}

const NO_MEMBERS: HomeView["members"] = [];

/** A flow's refusal, as the sentence the Rust side gave. */
const sentence = (e: unknown): string => (e instanceof Error ? e.message : String(e));

export function Panel({ bridge, home, call, menuOpen = false, platform = MAC }: Props) {
  const [dismissed, setDismissed] = useState<ReadonlySet<string>>(new Set());
  const [error, setError] = useState<string | null>(null);
  const [signingIn, setSigningIn] = useState(false);
  const [signInError, setSignInError] = useState<string | null>(null);
  const [guideError, setGuideError] = useState<string | null>(null);
  const [sharing, setSharing] = useState(false);
  const [asking, setAsking] = useState(false);

  const outcomes = useRingOutcomes(home?.members ?? NO_MEMBERS);
  const model = panelModel(home, call, { dismissed, error, signingIn, outcomes }, platform);

  /** Runs a flow; a refusal shows as the panel's one error line. */
  const run = useCallback((flow: () => Promise<unknown>) => {
    setError(null);
    flow().catch((e: unknown) => setError(sentence(e)));
  }, []);

  const dismiss = (text: string) => {
    setDismissed((d) => new Set([...d, text]));
    if (text === error) {
      setError(null);
    }
  };
  const lines: ShownLine[] =
    model.kind === "home"
      ? [
          ...(model.error === null ? [] : [{ text: model.error, tone: "bad" as const }]),
          ...(model.notice === null ? [] : [{ text: model.notice, tone: "plain" as const }]),
        ]
      : [];
  const hoverNotice = useNoticeClock(lines, dismiss);
  // A closed line that is no longer sent is forgotten, so the same words can show again.
  const present = [error, home?.error ?? null, home?.notice ?? null];
  const presentKey = present.join("\n");
  useEffect(() => {
    setDismissed((d) => forget(d, present));
    // `presentKey` stands for `present`.
  }, [presentKey]);

  const signIn = async () => {
    setSigningIn(true);
    setSignInError(null);
    try {
      await bridge.signIn();
    } catch (e) {
      const text = sentence(e);
      // Cancelling is what the person just did; no need to say so.
      setSignInError(text === "Sign-in was cancelled." ? null : text);
    } finally {
      setSigningIn(false);
    }
  };

  const share = () => {
    setSharing(true);
    setError(null);
    bridge
      .share()
      .catch((e: unknown) => setError(sentence(e)))
      .finally(() => setSharing(false));
  };

  const askToHost = () => {
    setAsking(true);
    setError(null);
    bridge
      .askToHost()
      .catch((e: unknown) => setError(sentence(e)))
      .finally(() => setAsking(false));
  };

  const signedIn = model.kind === "home";
  const menuItems: MenuItem[] = [
    ...(signedIn ? [{ label: "Sign out", run: () => run(() => bridge.signOut()) }] : []),
    { label: "Quit TwinCursor", run: () => void bridge.quit(), separated: signedIn },
  ];

  return (
    <div className="surface panel">
      <header className="panel-head">
        <span className="panel-brand">
          <Mark className="mark" />
          TwinCursor
        </span>
        {model.kind !== "loading" && <AccountMenu me={model.kind === "home" ? model.me : null} items={menuItems} startOpen={menuOpen} />}
      </header>

      {model.kind === "loading" && <div className="panel-loading" aria-busy="true" />}

      {model.kind === "off" && (
        <div className="panel-block">
          <p>This build isn't connected to a server.</p>
          <p className="muted">
            See the{" "}
            <button
              type="button"
              className="link-button"
              onClick={() => {
                setGuideError(null);
                bridge.openGuide().catch((e: unknown) => setGuideError(sentence(e)));
              }}
            >
              self-hosting guide
            </button>{" "}
            to set one up.
          </p>
          {guideError !== null && (
            <p className="panel-error" role="alert">
              {guideError}
            </p>
          )}
        </div>
      )}

      {model.kind === "signed_out" && (
        <div className="panel-block">
          {model.waiting ? (
            <>
              <p role="status">Finish signing in in your browser…</p>
              <button type="button" className="wide quiet" onClick={() => void bridge.cancelSignIn()}>
                Cancel
              </button>
            </>
          ) : (
            <>
              <p>{signInLine(platform)}</p>
              <button type="button" className="wide primary" onClick={() => void signIn()}>
                Sign in with GitHub
              </button>
            </>
          )}
          {signInError !== null && (
            <p className="panel-error" role="alert">
              {signInError}
            </p>
          )}
        </div>
      )}

      {model.kind === "home" && (
        <>
          <div className="panel-top">
            {model.rings.map((ring) => (
              <Ring key={ring.callId} ring={ring} answer={(accept) => run(() => bridge.answerRing(ring.callId, accept))} />
            ))}
            {model.needsScreenRecording && (
              <div className="panel-note" role="status">
                <span>TwinCursor needs Screen Recording to share your screen.</span>
                <button type="button" className="small" onClick={() => void bridge.openPane("screen_recording")}>
                  Open Settings
                </button>
              </div>
            )}
            {model.error !== null && <NoticeRow text={model.error} tone="bad" close={() => dismiss(model.error ?? "")} />}
            {model.notice !== null && (
              <NoticeRow
                text={model.notice}
                tone="plain"
                close={() => dismiss(model.notice ?? "")}
                hover={(over) => model.notice !== null && hoverNotice(model.notice, over)}
              />
            )}
            <Main
              main={model.main}
              sharing={sharing}
              share={share}
              asking={asking}
              ask={askToHost}
              paste={() => run(() => bridge.joinFromClipboard())}
              act={(op) => run(() => (op === "end" ? bridge.end() : bridge.cancelKnock()))}
            />
          </div>

          <section className="people" aria-labelledby="people-head">
            <h2 className="people-head" id="people-head">
              People
              {model.people.length > 0 && <span className="figure muted">{model.people.length}</span>}
            </h2>
            {model.people.length === 0 ? (
              <p className="people-empty">People you've been in a call with show up here.</p>
            ) : (
              <ul className="people-list">
                {model.people.map((p) => (
                  <Contact key={p.id} person={p} ring={() => run(() => bridge.ring(p.id))} />
                ))}
              </ul>
            )}
          </section>

          {model.serverProblem !== null && (
            <footer className="panel-foot" role="status">
              {model.serverProblem}
            </footer>
          )}
        </>
      )}
    </div>
  );
}

function AccountMenu({ me, items, startOpen }: { me: MeView | null; items: MenuItem[]; startOpen: boolean }) {
  const [open, setOpen] = useState(startOpen);
  const trigger = useRef<HTMLButtonElement>(null);
  const close = useCallback(() => setOpen(false), []);
  return (
    <div className="menu-anchor">
      <button
        ref={trigger}
        type="button"
        className={me === null ? "icon-button" : "avatar-button"}
        aria-haspopup="menu"
        aria-expanded={open}
        aria-controls={open ? "account-menu" : undefined}
        aria-label={me === null ? "More" : `${me.name}, account menu`}
        title={me === null ? "More" : me.name}
        onClick={() => setOpen((o) => !o)}
      >
        {me === null ? <MoreIcon /> : <Avatar face={me} size={24} />}
      </button>
      {open && (
        <Menu
          id="account-menu"
          label="Account"
          items={items}
          close={close}
          trigger={trigger}
          header={
            me === null ? undefined : (
              <div className="menu-head">
                <bdi className="menu-name">{me.name}</bdi>
                {me.handle !== null && <span className="muted">@{me.handle}</span>}
              </div>
            )
          }
        />
      )}
    </div>
  );
}

function Ring({ ring, answer }: { ring: RingCard; answer: (accept: boolean) => void }) {
  const id = `ring-${ring.callId}`;
  return (
    <div className="ring-card" role="alertdialog" aria-labelledby={id}>
      <Avatar face={ring} size={28} />
      <p className="ring-text" id={id}>
        <PhraseLine phrase={[{ name: ring.name }, " is calling"]} />
      </p>
      <button type="button" className="small quiet" onClick={() => answer(false)}>
        Decline
      </button>
      <button type="button" className="small primary" onClick={() => answer(true)}>
        Join
      </button>
    </div>
  );
}

function Main(props: {
  main: PanelMain;
  sharing: boolean;
  share: () => void;
  asking: boolean;
  ask: () => void;
  paste: () => void;
  act: (op: "end" | "cancel_knock") => void;
}) {
  const main = props.main;
  if (main.kind === "join") {
    return (
      <div className="panel-main">
        <p className="hint center" role="note">
          {main.note}
        </p>
        <button
          type="button"
          className={`wide${main.primary ? " primary" : ""}`}
          aria-describedby="paste-hint"
          aria-busy={main.busy}
          disabled={main.busy}
          onClick={props.paste}
        >
          Paste link
        </button>
        <p className="hint center" id="paste-hint">
          Join with a link someone sent you
        </p>
      </div>
    );
  }
  if (main.kind === "invite") {
    return (
      <div className="panel-main">
        <div className="panel-note invite">
          <span>You can join calls with a link. Sharing your own screen is by invitation on this server.</span>
          {main.asked ? (
            <span className="invite-asked muted" role="status">
              Asked. You'll be able to share once you're added.
            </span>
          ) : (
            <button
              type="button"
              className="small quiet"
              aria-busy={props.asking}
              onClick={props.asking ? undefined : props.ask}
            >
              Ask to be invited
            </button>
          )}
        </div>
        <button
          type="button"
          className={`wide${main.primary ? " primary" : ""}`}
          aria-describedby="paste-hint"
          aria-busy={main.busy}
          disabled={main.busy}
          onClick={props.paste}
        >
          Paste link
        </button>
        <p className="hint center" id="paste-hint">
          Join with a link someone sent you
        </p>
      </div>
    );
  }
  if (main.kind === "share") {
    const busy = main.busy || props.sharing;
    return (
      <div className="panel-main">
        <button
          type="button"
          className={`wide${main.primary ? " primary" : ""}`}
          aria-busy={busy}
          onClick={busy ? undefined : props.share}
        >
          {busy ? "Starting…" : "Share my screen"}
        </button>
        <button type="button" className="wide" aria-describedby="paste-hint" disabled={busy} onClick={props.paste}>
          Paste link
        </button>
        <p className="hint center" id="paste-hint">
          Join with a link someone sent you
        </p>
      </div>
    );
  }
  return (
    <div className="panel-call">
      <div className="panel-call-row">
        <span className={`dot ${main.dot}`} aria-hidden="true" />
        <PhraseLine phrase={main.line} className="panel-call-line" />
        {main.action !== null && (
          <button
            type="button"
            className={`small ${main.action.op === "end" ? "quiet-danger" : "quiet"}`}
            onClick={() => main.action !== null && props.act(main.action.op)}
          >
            {main.action.label}
          </button>
        )}
      </div>
      {main.link !== null && <LinkRow link={main.link} />}
    </div>
  );
}

function LinkRow({ link }: { link: string }) {
  const [copied, setCopied] = useState(false);
  const timer = useRef<number | null>(null);
  const copy = async () => {
    if (await copyText(link)) {
      setCopied(true);
      if (timer.current !== null) {
        window.clearTimeout(timer.current);
      }
      timer.current = window.setTimeout(() => setCopied(false), COPIED_MS);
    }
  };
  return (
    <div className="panel-call-row link">
      <code className="link-text" title={link}>
        {link}
      </code>
      <button type="button" className="small quiet copy" onClick={() => void copy()}>
        {copied ? "Copied" : "Copy link"}
      </button>
    </div>
  );
}

function Contact({ person, ring }: { person: PersonRow; ring: () => void }) {
  const action = person.action;
  return (
    <li className="contact">
      <Avatar face={person} size={28} />
      <span className="contact-names">
        <bdi className="contact-name" title={person.name}>
          {person.name}
        </bdi>
        {person.handle !== null && <span className="contact-handle">@{person.handle}</span>}
      </span>
      {action.kind === "call" && (
        <button
          type="button"
          className="small call"
          disabled={!action.enabled}
          aria-label={`Call ${person.name}`}
          onClick={ring}
        >
          Call
        </button>
      )}
      {action.kind === "status" && <span className="contact-status">{action.label}</span>}
    </li>
  );
}
