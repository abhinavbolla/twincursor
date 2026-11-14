// The app's Rust side: the commands it offers and the events it sends.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import type { StdinCloseResult } from "./checks";

export interface HelperInfo {
  /** Where the helper was found, or why it wasn't. */
  path: string | null;
  error: string | null;
  /** The helper sits inside the app bundle, so it gets the app's permissions. */
  in_bundle: boolean;
  app_pid: number;
}

export interface OutputLine {
  /** Which run of the helper printed it. */
  source: string;
  line: string;
}

export const helperInfo = (): Promise<HelperInfo> => invoke<HelperInfo>("helper_info");

/** Runs `tc-spike permissions`, asking macOS for what's missing when `request` is set. */
export const checkPermissions = (request: boolean): Promise<unknown> =>
  invoke<unknown>("check_permissions", { request });

/** Opens probe windows and runs `tc-spike permissions --capture --exclude-app-pid <app>`. */
export const checkCapture = (): Promise<unknown> => invoke<unknown>("check_capture");

/** Starts a helper, closes its stdin and times how long it takes to exit. */
export const checkStdinClose = (): Promise<StdinCloseResult> =>
  invoke<StdinCloseResult>("check_stdin_close");

/** Replaces the long-running helper with a fresh one, which sees grants given since. */
export const restartHelper = (): Promise<void> => invoke<void>("restart_helper");

export const onOutput = (f: (line: OutputLine) => void): Promise<UnlistenFn> =>
  listen<OutputLine>("helper-output", (e) => f(e.payload));

/** The long-running helper's latest status line, as JSON. */
export const onStatus = (f: (status: unknown) => void): Promise<UnlistenFn> =>
  listen<unknown>("helper-status", (e) => f(e.payload));
