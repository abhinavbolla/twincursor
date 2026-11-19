import { useState } from "react";

import type { EngineInfo } from "./bridge";
import type { CallState } from "./call";
import { copyText } from "./clipboard";

/** For bug reports: where the engine is, how it was started, and its recent log. */
export function Details({ info, call }: { info: EngineInfo | null; call: CallState }) {
  const [copied, setCopied] = useState(false);
  const text = [
    `engine: ${info?.path ?? info?.error ?? "?"}${info?.in_bundle ? " (in the app bundle)" : ""}`,
    call.args.length > 0 ? `started as: tc-engine ${call.args.join(" ")}` : null,
    ...call.logs,
  ]
    .filter((l): l is string => l !== null)
    .join("\n");
  return (
    <details className="details">
      <summary>Details</summary>
      <div className="row spread">
        <span className="muted">The engine's last {call.logs.length} log lines, for bug reports.</span>
        <button type="button" className="secondary small" onClick={() => void copyText(text).then(setCopied)}>
          {copied ? "Copied" : "Copy"}
        </button>
      </div>
      <pre className="log">{text}</pre>
    </details>
  );
}
