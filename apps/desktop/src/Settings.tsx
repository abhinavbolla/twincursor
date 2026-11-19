import { useEffect, useState } from "react";

import type { Bridge, EngineConfig, SecretChange, SettingsView } from "./bridge";

/** Server address, API key and API secret. The secret is written, never shown or read back. */
export function SettingsPanel({ bridge, inCall }: { bridge: Bridge; inCall: boolean }) {
  const [config, setConfig] = useState<EngineConfig | null>(null);
  const [saved, setSaved] = useState<SettingsView | null>(null);
  const [url, setUrl] = useState("");
  const [key, setKey] = useState("");
  const [secret, setSecret] = useState("");
  const [message, setMessage] = useState<string | null>(null);

  useEffect(() => {
    bridge.engineConfig().then(setConfig, (e: unknown) => setMessage(`Couldn't ask the engine: ${String(e)}`));
    bridge.readSettings().then(
      (view) => {
        setSaved(view);
        setUrl(view.url ?? "");
        setKey(view.api_key ?? "");
      },
      (e: unknown) => setMessage(String(e)),
    );
  }, [bridge]);

  const save = async (secretChange: SecretChange) => {
    try {
      const view = await bridge.saveSettings({
        url: url.trim() === "" ? null : url.trim(),
        api_key: key.trim() === "" ? null : key.trim(),
        api_secret: secretChange,
      });
      setSaved(view);
      setSecret("");
      setMessage(inCall ? "Saved. They apply from the next call." : "Saved.");
    } catch (e) {
      setMessage(String(e));
    }
  };

  const builtIn =
    config === null
      ? "…"
      : config.source === "dev"
        ? "the local dev server (this build names no server)"
        : `${config.url}, key ${config.api_key}, secret ${config.has_secret ? "built in" : "none"}`;

  return (
    <section className="card settings">
      <h2>Server</h2>
      <p className="hint">Built in: {builtIn}. Leave a field empty to use what's built in.</p>
      <label className="field">
        <span>Server address</span>
        <input
          value={url}
          spellCheck={false}
          autoComplete="off"
          placeholder={config?.url ?? "wss://…"}
          onChange={(e) => setUrl(e.target.value)}
        />
      </label>
      <label className="field">
        <span>API key</span>
        <input
          value={key}
          spellCheck={false}
          autoComplete="off"
          placeholder={config?.api_key ?? ""}
          onChange={(e) => setKey(e.target.value)}
        />
      </label>
      <label className="field">
        <span>API secret</span>
        <input
          type="password"
          value={secret}
          autoComplete="off"
          placeholder={
            saved?.has_secret ? "Saved (type to replace)" : config?.has_secret ? "Built in" : "None"
          }
          onChange={(e) => setSecret(e.target.value)}
        />
      </label>
      <p className="hint">
        Saved in this Mac's app settings, readable only by you, and handed to the engine privately.
        The secret isn't shown again after saving.
      </p>
      <div className="row">
        <button
          type="button"
          onClick={() => void save(secret === "" ? { do: "keep" } : { do: "set", value: secret })}
        >
          Save
        </button>
        {saved?.has_secret === true && (
          <button type="button" className="secondary" onClick={() => void save({ do: "clear" })}>
            Forget the saved secret
          </button>
        )}
      </div>
      {message !== null && <p role="status">{message}</p>}
    </section>
  );
}
