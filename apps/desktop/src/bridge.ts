// The app's Rust side, as the window uses it. Inside the app this is Tauri; in a browser during
// development (`npm run dev`) it is a mock engine, so the window can be tried without a call.
// The mock is imported only when `import.meta.env.DEV` is true, so a production build (the
// bundled app) doesn't contain it.

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { ComponentType } from "react";

import type { EngineCommand, EngineOutput, Plan } from "./ipc";
import { parseOutput } from "./ipc";
import type { PermissionName } from "./permissions";

export interface EngineInfo {
  /** Where the engine was found, or why it wasn't. */
  path: string | null;
  error: string | null;
  /** Inside the app bundle, so it gets the app's permissions. */
  in_bundle: boolean;
  app_pid: number;
}

/** The server overrides, as the window may see them: whether a secret is saved, never what. */
export interface SettingsView {
  url: string | null;
  api_key: string | null;
  has_secret: boolean;
}

export type SecretChange = { do: "keep" } | { do: "clear" } | { do: "set"; value: string };

export interface SettingsChange {
  url: string | null;
  api_key: string | null;
  api_secret: SecretChange;
}

/** `tc-engine config`: the server the build names, or the local dev server. */
export interface EngineConfig {
  url: string;
  api_key: string;
  has_secret: boolean;
  source: string;
}

export interface Bridge {
  engineInfo(): Promise<EngineInfo>;
  startCall(plan: Plan): Promise<void>;
  send(command: EngineCommand): Promise<void>;
  leave(): Promise<void>;
  /** A fresh `tc-engine permissions` (with `request`, asks macOS for what's missing). */
  permissions(request: boolean): Promise<unknown>;
  openPane(name: PermissionName): Promise<void>;
  readSettings(): Promise<SettingsView>;
  saveSettings(change: SettingsChange): Promise<SettingsView>;
  engineConfig(): Promise<EngineConfig>;
  /** Everything the Rust side says about the call. Returns a function that stops listening. */
  onOutput(f: (output: EngineOutput) => void): () => void;
}

export interface LoadedBridge {
  bridge: Bridge;
  /** The mock's control panel, in development only. */
  DevPanel: ComponentType | null;
}

function tauriBridge(): Bridge {
  return {
    engineInfo: () => invoke<EngineInfo>("engine_info"),
    startCall: (plan) => invoke<void>("start_call", { plan }),
    send: (command) => invoke<void>("send_command", { command }),
    leave: () => invoke<void>("leave_call"),
    permissions: (request) => invoke<unknown>("permissions", { request }),
    openPane: (pane) => invoke<void>("open_settings_pane", { pane }),
    readSettings: () => invoke<SettingsView>("read_settings"),
    saveSettings: (change) => invoke<SettingsView>("save_settings", { change }),
    engineConfig: () => invoke<EngineConfig>("engine_config"),
    onOutput: (f) => {
      const stop = listen<unknown>("engine", (e) => {
        const output = parseOutput(e.payload);
        if (output !== null) {
          f(output);
        }
      });
      return () => void stop.then((unlisten) => unlisten());
    },
  };
}

const inTauri = (): boolean => "__TAURI_INTERNALS__" in window;

/** The bridge for where this page runs, or null in a production build outside the app. */
export async function loadBridge(): Promise<LoadedBridge | null> {
  if (inTauri()) {
    return { bridge: tauriBridge(), DevPanel: null };
  }
  if (import.meta.env.DEV) {
    const mock = await import("./mock");
    return mock.createMock(new URLSearchParams(window.location.search));
  }
  return null;
}
