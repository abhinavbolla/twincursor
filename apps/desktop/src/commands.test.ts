import { describe, expect, it } from "vitest";

import commandLines from "../../../crates/ipc/fixtures/commands.jsonl?raw";
import * as build from "./commands";
import type { EngineCommand } from "./ipc";

const fixtures = commandLines.split("\n").filter((l) => l.trim() !== "");

/** The builder for a fixture, by its `cmd`. */
function rebuild(fixture: Record<string, unknown>): EngineCommand {
  const request = Number(fixture.request);
  const device = (fixture.device ?? null) as string | null;
  switch (fixture.cmd) {
    case "mute":
      return build.mute();
    case "unmute":
      return build.unmute();
    case "leave":
      return build.leave();
    case "allow_control":
      return build.allowControl(request);
    case "deny_control":
      return build.denyControl(request);
    case "revoke_control":
      return build.revokeControl();
    case "trust_control":
      return build.trustControl(String(fixture.identity));
    case "trust_on_arrival":
      return build.trustOnArrival(String(fixture.identity));
    case "request_control":
      return build.requestControl();
    case "release_control":
      return build.releaseControl();
    case "set_mic":
      return build.setMic(device);
    case "set_speaker":
      return build.setSpeaker(device);
    default:
      throw new Error(`no builder for ${String(fixture.cmd)}`);
  }
}

describe("commands", () => {
  it("are written exactly as the engine's fixtures", () => {
    expect(fixtures.length).toBeGreaterThanOrEqual(10);
    for (const line of fixtures) {
      expect(JSON.stringify(rebuild(JSON.parse(line) as Record<string, unknown>))).toBe(line);
    }
  });

  it("answer the request they name", () => {
    expect(build.allowControl(7)).toEqual({ cmd: "allow_control", request: 7 });
    expect(build.denyControl(8)).toEqual({ cmd: "deny_control", request: 8 });
    expect(build.trustControl("guest-1")).toEqual({ cmd: "trust_control", identity: "guest-1" });
    expect(build.trustOnArrival("guest-2")).toEqual({ cmd: "trust_on_arrival", identity: "guest-2" });
  });
});
