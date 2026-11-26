// Builders for the commands the window sends the engine (docs/ipc.md).

import type { EngineCommand } from "./ipc";

export const mute = (): EngineCommand => ({ cmd: "mute" });
export const unmute = (): EngineCommand => ({ cmd: "unmute" });
export const leave = (): EngineCommand => ({ cmd: "leave" });
/** Grants request `request`, and only that one: the number from its `control_asked`. */
export const allowControl = (request: number): EngineCommand => ({ cmd: "allow_control", request });
export const denyControl = (request: number): EngineCommand => ({ cmd: "deny_control", request });
/** Takes control back, and every trust with it. */
export const revokeControl = (): EngineCommand => ({ cmd: "revoke_control" });
/** Lets this guest take control during this call without being asked again, until a revoke. */
export const trustControl = (identity: string): EngineCommand => ({ cmd: "trust_control", identity });
export const requestControl = (): EngineCommand => ({ cmd: "request_control" });
export const releaseControl = (): EngineCommand => ({ cmd: "release_control" });
/** A device by name as `tc-engine devices` lists it, or null for the system default. */
export const setMic = (device: string | null): EngineCommand => ({ cmd: "set_mic", device });
export const setSpeaker = (device: string | null): EngineCommand => ({ cmd: "set_speaker", device });
