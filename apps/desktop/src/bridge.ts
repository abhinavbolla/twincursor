// The app's Rust side, as the window uses it. Inside the app this is Tauri; in a browser during
// development (`npm run dev`) it is a mock engine, so the window can be tried without a call.
// The mock is imported only when `import.meta.env.DEV` is true, so a production build (the
// bundled app) doesn't contain it.

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { ComponentType } from "react";

import { parseAccountState, type AccountState } from "./accountState";
import { parseHomeView, type HomeView, type NeededPermission } from "./home";
import type { EngineCommand, EngineOutput } from "./ipc";
import { parseOutput } from "./ipc";
import type { PermissionName } from "./permissions";
import { parsePlatform, type PlatformInfo } from "./platform";

export interface EngineInfo {
  /** Where the engine was found, or why it wasn't. */
  path: string | null;
  error: string | null;
  /** Inside the app bundle, so it gets the app's permissions. */
  in_bundle: boolean;
  app_pid: number;
}

export interface Bridge {
  engineInfo(): Promise<EngineInfo>;
  send(command: EngineCommand): Promise<void>;
  leave(): Promise<void>;
  /** A fresh `tc-engine permissions` (with `request`, asks macOS for what's missing). */
  permissions(request: boolean): Promise<unknown>;
  openPane(name: PermissionName): Promise<void>;
  /** Who is signed in. Never a token: the Rust side keeps the session. */
  accountState(): Promise<AccountState>;
  /** Opens GitHub sign-in in the browser; resolves once signed in, rejects with a sentence. */
  signIn(): Promise<AccountState>;
  /** Stops a sign-in waiting for the browser (its signIn() then rejects). */
  cancelSignIn(): Promise<void>;
  signOut(): Promise<AccountState>;
  /** Everything the Rust side says about the call. Returns a function that stops listening. */
  onOutput(f: (output: EngineOutput) => void): () => void;

  // Calls with accounts. Each flow resolves with the home view after it, or rejects with a
  // sentence. The Rust side keeps every token and pass, and reads the clipboard itself.

  /** The home view now (what a window opened later starts from). */
  homeState(): Promise<HomeView>;
  /** The current call's engine output, to replay through `callReducer` in a late window. */
  callSnapshot(): Promise<EngineOutput[]>;
  /** Share my screen; the call link goes on the clipboard. */
  share(): Promise<HomeView>;
  /** Paste link: the Rust side reads the clipboard; only the outcome comes back. */
  joinFromClipboard(): Promise<HomeView>;
  joinLink(text: string): Promise<HomeView>;
  cancelKnock(): Promise<HomeView>;
  /** Refused with "Too soon. Try again." within a second of the knock being shown. */
  answerKnock(userId: string, letIn: boolean, control: boolean): Promise<HomeView>;
  giveControl(identity: string): Promise<HomeView>;
  /** Give control to `home.owed`: refused with "Too soon. Try again." until it is armed. */
  giveOwedControl(): Promise<HomeView>;
  /** Reset and ask again: clears TwinCursor's own entry for this permission in macOS's list and
   * asks macOS again. Only while `home.needs` is that permission. */
  resetPermission(which: NeededPermission): Promise<HomeView>;
  ring(userId: string): Promise<HomeView>;
  answerRing(callId: string, accept: boolean): Promise<HomeView>;
  /** Ask to be invited, where this person may not start calls (`account.can_host` is false). */
  askToHost(): Promise<HomeView>;
  /** Stop sharing, or leave. */
  end(): Promise<HomeView>;
  /** The home view on every change. Returns a function that stops listening. */
  onHome(f: (home: HomeView) => void): () => void;
  /** Quits the app (the panel's Quit TwinCursor). */
  quit(): Promise<void>;
  /** Opens the self-hosting guide in the browser. The address is fixed on the Rust side. */
  openGuide(): Promise<void>;
  /** What this platform can do: only a Mac shares (and so calls people). */
  platformInfo(): Promise<PlatformInfo>;
}

export interface LoadedBridge {
  bridge: Bridge;
  /** The mock's control panel, in development only. */
  DevPanel: ComponentType | null;
}

/** The Rust side's account answer, checked. */
function accountAnswer(value: unknown): AccountState {
  const state = parseAccountState(value);
  if (state === null) {
    throw new Error("The app's account answer couldn't be read.");
  }
  return state;
}

/** The Rust side's home view, checked. */
export function homeAnswer(value: unknown): HomeView {
  const home = parseHomeView(value);
  if (home === null) {
    throw new Error("The app's answer about calls couldn't be read.");
  }
  return home;
}

/** The Rust side's platform answer, checked. */
function platformAnswer(value: unknown): PlatformInfo {
  const platform = parsePlatform(value);
  if (platform === null) {
    throw new Error("The app's platform answer couldn't be read.");
  }
  return platform;
}

/** The Rust side's call snapshot, checked. */
export function snapshotAnswer(value: unknown): EngineOutput[] {
  if (!Array.isArray(value)) {
    throw new Error("The app's call snapshot couldn't be read.");
  }
  return value.map((v) => {
    const output = parseOutput(v);
    if (output === null) {
      throw new Error("The app's call snapshot couldn't be read.");
    }
    return output;
  });
}

function tauriBridge(): Bridge {
  return {
    engineInfo: () => invoke<EngineInfo>("engine_info"),
    send: (command) => invoke<void>("send_command", { command }),
    leave: () => invoke<void>("leave_call"),
    permissions: (request) => invoke<unknown>("permissions", { request }),
    openPane: (pane) => invoke<void>("open_settings_pane", { pane }),
    accountState: () => invoke<unknown>("account_state").then(accountAnswer),
    signIn: () => invoke<unknown>("sign_in").then(accountAnswer),
    cancelSignIn: () => invoke<void>("cancel_sign_in"),
    signOut: () => invoke<unknown>("sign_out").then(accountAnswer),
    onOutput: (f) => {
      const stop = listen<unknown>("engine", (e) => {
        const output = parseOutput(e.payload);
        if (output !== null) {
          f(output);
        }
      });
      return () => void stop.then((unlisten) => unlisten());
    },
    homeState: () => invoke<unknown>("home_state").then(homeAnswer),
    callSnapshot: () => invoke<unknown>("call_snapshot").then(snapshotAnswer),
    share: () => invoke<unknown>("share").then(homeAnswer),
    joinFromClipboard: () => invoke<unknown>("join_from_clipboard").then(homeAnswer),
    joinLink: (text) => invoke<unknown>("join_link", { text }).then(homeAnswer),
    cancelKnock: () => invoke<unknown>("cancel_knock").then(homeAnswer),
    answerKnock: (userId, letIn, control) =>
      invoke<unknown>("answer_knock", { user_id: userId, let_in: letIn, control }).then(homeAnswer),
    giveControl: (identity) => invoke<unknown>("give_control", { identity }).then(homeAnswer),
    giveOwedControl: () => invoke<unknown>("give_owed_control").then(homeAnswer),
    resetPermission: (which) => invoke<unknown>("reset_permission", { which }).then(homeAnswer),
    ring: (userId) => invoke<unknown>("ring", { user_id: userId }).then(homeAnswer),
    answerRing: (callId, accept) =>
      invoke<unknown>("answer_ring", { call_id: callId, accept }).then(homeAnswer),
    askToHost: () => invoke<unknown>("ask_to_host").then(homeAnswer),
    end: () => invoke<unknown>("end").then(homeAnswer),
    onHome: (f) => {
      const stop = listen<unknown>("home", (e) => {
        const home = parseHomeView(e.payload);
        if (home !== null) {
          f(home);
        }
      });
      return () => void stop.then((unlisten) => unlisten());
    },
    quit: () => invoke<void>("quit"),
    openGuide: () => invoke<void>("open_self_hosting_guide"),
    platformInfo: () => invoke<unknown>("platform_info").then(platformAnswer),
  };
}

export const inTauri = (): boolean => "__TAURI_INTERNALS__" in window;

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
