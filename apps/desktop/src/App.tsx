import { useCallback, useEffect, useRef, useState } from "react";

import {
  checkCapture,
  checkPermissions,
  checkStdinClose,
  helperInfo,
  onOutput,
  onStatus,
  restartHelper,
  type HelperInfo,
  type OutputLine,
} from "./bridge";
import {
  captureChecks,
  describeAccess,
  failed,
  parseReport,
  permissionChecks,
  stdinCloseCheck,
  type CheckResult,
  type PermissionStatus,
} from "./checks";

/** Output lines kept on screen. */
const MAX_LINES = 400;

function Verdict({ result }: { result: CheckResult }) {
  return (
    <li className={`check ${result.verdict}`}>
      <span className="mark">{result.verdict === "pass" ? "Pass" : "Fail"}</span>
      <span className="label">{result.label}</span>
      <span className="detail">{result.detail}</span>
    </li>
  );
}

function LiveStatus({ status }: { status: PermissionStatus | null }) {
  if (status === null) {
    return <p className="muted">Waiting for the helper…</p>;
  }
  const rows: [string, PermissionStatus["screen_recording"]][] = [
    ["Screen Recording", status.screen_recording],
    ["Accessibility", status.accessibility],
    ["Microphone", status.microphone],
  ];
  return (
    <ul className="live">
      {rows.map(([name, access]) => (
        <li key={name} className={access === "granted" ? "on" : "off"}>
          {name}: {describeAccess(access)}
        </li>
      ))}
    </ul>
  );
}

export function App() {
  const [info, setInfo] = useState<HelperInfo | null>(null);
  const [status, setStatus] = useState<PermissionStatus | null>(null);
  const [results, setResults] = useState<CheckResult[]>([]);
  const [running, setRunning] = useState(false);
  const [lines, setLines] = useState<OutputLine[]>([]);
  const logEnd = useRef<HTMLDivElement>(null);

  useEffect(() => {
    helperInfo().then(setInfo, (e: unknown) =>
      setInfo({ path: null, error: String(e), in_bundle: false, app_pid: 0 }),
    );
    const output = onOutput((line) => setLines((all) => [...all, line].slice(-MAX_LINES)));
    const live = onStatus((value) => {
      try {
        setStatus(parseReport(value));
      } catch {
        // A status this app doesn't understand: leave the last one up.
      }
    });
    return () => {
      void output.then((stop) => stop());
      void live.then((stop) => stop());
    };
  }, []);

  useEffect(() => {
    logEnd.current?.scrollIntoView({ block: "end" });
  }, [lines]);

  const runChecks = useCallback(async () => {
    setRunning(true);
    const done: CheckResult[] = [];
    const add = (more: CheckResult[]) => {
      done.push(...more);
      setResults([...done]);
    };
    setResults([]);
    try {
      add(permissionChecks(parseReport(await checkPermissions(true))));
    } catch (e) {
      add([failed("permissions", "Read the helper's permissions", e)]);
    }
    try {
      add(captureChecks(parseReport(await checkCapture()).capture));
    } catch (e) {
      add([failed("capture", "The helper can capture the screen", e)]);
    }
    try {
      add([stdinCloseCheck(await checkStdinClose())]);
    } catch (e) {
      add([failed("stdin", "The helper exits when the app goes away", e)]);
    }
    // Each check above ran in a fresh helper; the live status line gets one too.
    await restartHelper().catch(() => undefined);
    setRunning(false);
  }, []);

  const passed = results.filter((r) => r.verdict === "pass").length;

  return (
    <main>
      <header>
        <h1>TwinCursor</h1>
        <p className="muted">
          This window starts the TwinCursor helper, which does the capture, voice and remote
          control, and checks that macOS gives it TwinCursor's permissions.
        </p>
      </header>

      <section>
        <h2>Helper</h2>
        {info === null ? (
          <p className="muted">Looking for the helper…</p>
        ) : info.path === null ? (
          <p className="bad">Helper not found: {info.error}</p>
        ) : (
          <p>
            <code>{info.path}</code>
            <br />
            <span className={info.in_bundle ? "muted" : "bad"}>
              {info.in_bundle
                ? "Inside the app bundle."
                : "Not inside an app bundle (a development build): macOS gives it the permissions of whatever started the app."}
            </span>
          </p>
        )}
        <LiveStatus status={status} />
      </section>

      <section>
        <h2>Checks</h2>
        <p className="muted">
          Press the button and answer each macOS prompt with Allow, or turn TwinCursor on in
          the System Settings pane it opens. Two magenta squares appear for a few seconds while
          the capture is checked: leave them uncovered. Every press starts fresh helpers, so
          after allowing something, press the button again to see it take effect. If macOS
          offers to quit and reopen TwinCursor after Screen Recording, choose Later first and
          press the button again: the check shows whether a fresh helper gets the grant
          without restarting the app.
        </p>
        <button type="button" onClick={() => void runChecks()} disabled={running || info?.path == null}>
          {running ? "Running…" : "Run checks"}
        </button>
        {results.length > 0 && (
          <>
            <ul className="checks">
              {results.map((r) => (
                <Verdict key={r.id} result={r} />
              ))}
            </ul>
            {!running && (
              <p className={passed === results.length ? "good" : "bad"}>
                {passed} of {results.length} passed.
              </p>
            )}
          </>
        )}
      </section>

      <section className="output">
        <h2>Helper output</h2>
        <div className="log">
          {lines.map((l, i) => (
            <div key={i}>
              <span className="source">{l.source}</span> {l.line}
            </div>
          ))}
          <div ref={logEnd} />
        </div>
      </section>
    </main>
  );
}
