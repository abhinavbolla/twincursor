import { lazy, Suspense, useCallback, useEffect, useReducer, useState } from "react";

import { AccountCard } from "./Account";
import type { EngineInfo, LoadedBridge } from "./bridge";
import { inTauri, loadBridge } from "./bridge";
import { CallBarSurface } from "./CallBar";
import { callReducer, initialCall } from "./call";
import { CallScreen } from "./CallScreen";
import { Details } from "./Details";
import { Mark } from "./icons";
import type { EngineCommand, Plan } from "./ipc";
import { PanelSurface } from "./Panel";
import { parsePermissions, type PermissionStatus } from "./permissions";
import { SettingsPanel } from "./Settings";
import { StartScreen } from "./StartScreen";

/** How often the permissions are read again while the start screen is showing. */
const PERMISSIONS_EVERY_MS = 2000;

function usePageVisible(): boolean {
  const [visible, setVisible] = useState(document.visibilityState === "visible");
  useEffect(() => {
    const update = () => setVisible(document.visibilityState === "visible");
    document.addEventListener("visibilitychange", update);
    return () => document.removeEventListener("visibilitychange", update);
  }, []);
  return visible;
}

function Window({ bridge, DevPanel }: LoadedBridge) {
  const [call, dispatch] = useReducer(callReducer, initialCall);
  const [info, setInfo] = useState<EngineInfo | null>(null);
  const [permissions, setPermissions] = useState<PermissionStatus | null>(null);
  const [permissionsError, setPermissionsError] = useState<string | null>(null);
  const [showSettings, setShowSettings] = useState(false);
  const [busy, setBusy] = useState(false);
  const visible = usePageVisible();
  const inCall = call.phase === "in_call";

  useEffect(() => bridge.onOutput((output) => dispatch({ type: "output", output })), [bridge]);

  useEffect(() => {
    bridge.engineInfo().then(setInfo, (e: unknown) =>
      setInfo({ path: null, error: String(e), in_bundle: false, app_pid: 0 }),
    );
  }, [bridge]);

  const readPermissions = useCallback(
    async (request: boolean) => {
      try {
        const status = parsePermissions(await bridge.permissions(request));
        setPermissions(status);
        setPermissionsError(status === null ? "The engine's answer couldn't be read." : null);
      } catch (e) {
        setPermissionsError(String(e));
      }
    },
    [bridge],
  );

  // A fresh engine process every couple of seconds: macOS applies a new grant only to processes
  // started after it. Not during a call, and not while the window is hidden.
  useEffect(() => {
    if (inCall || !visible || info?.path == null) {
      return;
    }
    void readPermissions(false);
    const timer = window.setInterval(() => void readPermissions(false), PERMISSIONS_EVERY_MS);
    return () => window.clearInterval(timer);
  }, [inCall, visible, info, readPermissions]);

  const start = useCallback(
    async (plan: Plan) => {
      setBusy(true);
      try {
        await bridge.startCall(plan);
      } catch (e) {
        dispatch({ type: "start_failed", message: String(e) });
      } finally {
        setBusy(false);
      }
    },
    [bridge],
  );

  const send = useCallback(
    (command: EngineCommand) => {
      bridge.send(command).catch((e: unknown) =>
        dispatch({ type: "output", output: { kind: "log", line: `(not sent: ${String(e)})` } }),
      );
    },
    [bridge],
  );

  return (
    <main>
      <header className="top">
        <h1 className="brand">
          <Mark />
          TwinCursor
        </h1>
        <button
          type="button"
          className="small"
          aria-expanded={showSettings}
          onClick={() => setShowSettings((s) => !s)}
        >
          {showSettings ? "Close settings" : "Settings"}
        </button>
      </header>

      {showSettings && <SettingsPanel bridge={bridge} inCall={inCall} />}

      {!inCall && <AccountCard bridge={bridge} />}

      {info !== null && info.path === null && (
        <p className="banner bad" role="alert">
          The engine wasn't found, so calls can't start: {info.error}
        </p>
      )}

      {inCall ? (
        <CallScreen
          call={call}
          send={send}
          leave={() => void bridge.leave()}
          answered={() => dispatch({ type: "answered" })}
          dismiss={() => dispatch({ type: "dismiss" })}
        />
      ) : (
        <StartScreen
          endMessage={call.endMessage}
          dismiss={() => dispatch({ type: "dismiss" })}
          start={(plan) => void start(plan)}
          busy={busy || info?.path == null}
          permissions={permissions}
          permissionsError={permissionsError}
          ask={() => void readPermissions(true)}
          openPane={(name) => void bridge.openPane(name)}
        />
      )}

      <Details info={info} call={call} />
      {DevPanel !== null && <DevPanel />}
    </main>
  );
}

/** The dev page for the panel and the bar: in a browser under `vite` only. */
const DevPage = import.meta.env.DEV ? lazy(() => import("./devpage").then((m) => ({ default: m.DevPage }))) : null;

/** `?view=panel` or `?view=bar`: one surface alone, as its own native window shows it. */
function Surface({ view }: { view: "panel" | "bar" }) {
  const [loaded, setLoaded] = useState<LoadedBridge | null | undefined>(undefined);
  useEffect(() => {
    loadBridge().then(setLoaded, () => setLoaded(null));
  }, []);
  if (loaded === undefined || loaded === null) {
    return null;
  }
  return view === "panel" ? <PanelSurface bridge={loaded.bridge} /> : <CallBarSurface bridge={loaded.bridge} />;
}

export function App() {
  const params = new URLSearchParams(window.location.search);
  const view = params.get("view");
  if (view !== null) {
    if (inTauri()) {
      return view === "bar" ? <Surface view="bar" /> : <Surface view="panel" />;
    }
    if (DevPage !== null) {
      return (
        <Suspense fallback={null}>
          <DevPage params={params} />
        </Suspense>
      );
    }
  }
  return <WindowApp />;
}

function WindowApp() {
  const [loaded, setLoaded] = useState<LoadedBridge | null | undefined>(undefined);
  useEffect(() => {
    loadBridge().then(setLoaded, () => setLoaded(null));
  }, []);
  if (loaded === undefined) {
    return null;
  }
  if (loaded === null) {
    return (
      <main>
        <h1>TwinCursor</h1>
        <p>This page runs inside the TwinCursor app.</p>
      </main>
    );
  }
  return <Window bridge={loaded.bridge} DevPanel={loaded.DevPanel} />;
}
