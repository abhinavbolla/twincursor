import { describe, expect, it } from "vitest";

import fixtureText from "../src-tauri/fixtures/home-view.json?raw";
import { homeAnswer, snapshotAnswer } from "./bridge";
import { parseHomeView, type HomeView } from "./home";

const fixture = JSON.parse(fixtureText) as HomeView;

describe("home view", () => {
  it("reads what the Rust side sends", () => {
    const home = parseHomeView(fixture);
    expect(home).not.toBeNull();
    expect(home?.stage).toEqual({
      kind: "hosting",
      call_id: "11111111-1111-4111-8111-111111111111",
      link: "twincursor://call/0123456789abcdef0123456789abcdef",
    });
    expect(home?.knocks[0]?.armed).toBe(false);
    expect(home?.members[0]?.state).toBe("admitted");
    expect(home?.needs).toBe("accessibility");
    expect(home?.stuck).toBe(true);
    expect(home?.owed).toEqual({ identity: "guest-00000000-0000-4000-8000-00000000000b", name: "Sam", armed: false });
    expect(parseHomeView({ ...fixture, owed: null })?.owed).toBeNull();
    expect(home).toEqual(fixture);
  });

  it("reads every stage", () => {
    const host = { id: "u2", name: "Sam", handle: null, avatar_url: null };
    for (const stage of [
      { kind: "idle" },
      { kind: "starting" },
      { kind: "ending" },
      { kind: "knocking", host },
      { kind: "joined", host },
    ]) {
      expect(parseHomeView({ ...fixture, stage })?.stage).toEqual(stage);
    }
  });

  it("refuses what isn't a home view", () => {
    expect(parseHomeView(null)).toBeNull();
    expect(parseHomeView({ ...fixture, stage: { kind: "hosting" } })).toBeNull();
    expect(parseHomeView({ ...fixture, stage: { kind: "elsewhere" } })).toBeNull();
    expect(parseHomeView({ ...fixture, knocks: [{ user_id: "u", name: "K", handle: null, avatar_url: null }] })).toBeNull();
    expect(parseHomeView({ ...fixture, members: [{ ...fixture.members[0], state: "beamed_in" }] })).toBeNull();
    expect(parseHomeView({ ...fixture, needs: "camera" })).toBeNull();
    expect(parseHomeView({ ...fixture, stuck: undefined })).toBeNull();
    expect(parseHomeView({ ...fixture, owed: { identity: "guest-x", name: "Sam" } })).toBeNull();
    expect(parseHomeView({ ...fixture, account: { signed_in: true } })).toBeNull();
    expect(parseHomeView({ ...fixture, contacts: null })).toBeNull();
  });

  it("checks the bridge's answers", () => {
    expect(homeAnswer(fixture)).toEqual(fixture);
    expect(() => homeAnswer({ stage: "idle" })).toThrow("couldn't be read");
    expect(snapshotAnswer([{ kind: "exited", code: 0 }])).toEqual([{ kind: "exited", code: 0 }]);
    expect(() => snapshotAnswer([{ kind: "exited", code: "0" }])).toThrow("couldn't be read");
    expect(() => snapshotAnswer({})).toThrow("couldn't be read");
  });
});
