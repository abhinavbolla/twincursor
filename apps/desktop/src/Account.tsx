import { useCallback, useEffect, useState } from "react";

import { accountView, type AccountState } from "./accountState";
import type { Bridge } from "./bridge";

/** Who is signed in, and signing in and out. Shows nothing when the build has no backend. */
export function AccountCard({ bridge }: { bridge: Bridge }) {
  const [state, setState] = useState<AccountState | null>(null);
  const [waiting, setWaiting] = useState(false);
  const [message, setMessage] = useState<string | null>(null);

  useEffect(() => {
    bridge.accountState().then(setState, () => setState(null));
  }, [bridge]);

  const signIn = useCallback(async () => {
    setWaiting(true);
    setMessage(null);
    try {
      setState(await bridge.signIn());
    } catch (e) {
      const text = String(e);
      // Cancelling is what the person just did; no need to say so.
      setMessage(text === "Sign-in was cancelled." ? null : text);
      bridge.accountState().then(setState, () => undefined);
    } finally {
      setWaiting(false);
    }
  }, [bridge]);

  const signOut = useCallback(async () => {
    setMessage(null);
    try {
      setState(await bridge.signOut());
    } catch (e) {
      setMessage(String(e));
    }
  }, [bridge]);

  const view = accountView(state, waiting);
  if (view.kind === "hidden") {
    return null;
  }
  return (
    <section className="card account" aria-label="Account">
      {view.kind === "signed_in" ? (
        <div className="row spread">
          <div className="row">
            {view.avatar !== null && (
              <img className="avatar" src={view.avatar} alt="" width={32} height={32} />
            )}
            <div className="stack">
              <span>{view.name}</span>
              {view.handle !== null && <span className="muted">@{view.handle}</span>}
            </div>
          </div>
          <button type="button" className="small" onClick={() => void signOut()}>
            Sign out
          </button>
        </div>
      ) : view.kind === "waiting" ? (
        <div className="row spread">
          <span className="muted" role="status">
            Finish signing in in your browser…
          </span>
          <button type="button" className="small" onClick={() => void bridge.cancelSignIn()}>
            Cancel
          </button>
        </div>
      ) : (
        <div className="row">
          <button type="button" className="primary" onClick={() => void signIn()}>
            Sign in with GitHub
          </button>
        </div>
      )}
      {message !== null && (
        <p className="bad" role="alert">
          {message}
        </p>
      )}
    </section>
  );
}
