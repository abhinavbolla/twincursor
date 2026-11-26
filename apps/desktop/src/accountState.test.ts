import { describe, expect, it } from "vitest";

import { accountView, parseAccountState, safeAvatar, type AccountState } from "./accountState";

const signedIn: AccountState = {
  configured: true,
  signed_in: true,
  me: {
    id: "u1",
    name: "Alex",
    handle: "alex",
    avatar_url: "https://avatars.githubusercontent.com/u/1?v=4",
  },
  signing_in: false,
  can_host: true,
  asked_to_host: false,
};
const OFF = { configured: false, signed_in: false, me: null, signing_in: false, can_host: true, asked_to_host: false };

describe("account", () => {
  it("reads account_state", () => {
    expect(parseAccountState(signedIn)).toEqual(signedIn);
    expect(parseAccountState(OFF)).toEqual(OFF);
    expect(parseAccountState({ ...signedIn, can_host: false, asked_to_host: true })).toEqual({
      ...signedIn,
      can_host: false,
      asked_to_host: true,
    });
    expect(parseAccountState(null)).toBeNull();
    expect(parseAccountState({ ...OFF, configured: "yes" })).toBeNull();
    // The Rust side always says whether they may host.
    expect(parseAccountState({ ...signedIn, can_host: undefined })).toBeNull();
    expect(parseAccountState({ ...signedIn, asked_to_host: "yes" })).toBeNull();
    expect(parseAccountState({ ...signedIn, me: { id: 1, name: "Alex" } })).toBeNull();
  });

  it("shows nothing when no backend is set up", () => {
    expect(accountView(null, false)).toEqual({ kind: "hidden" });
    expect(
      accountView(OFF, true),
    ).toEqual({ kind: "hidden" });
  });

  it("offers sign-in, then waits for the browser", () => {
    const out: AccountState = { ...OFF, configured: true };
    expect(accountView(out, false)).toEqual({ kind: "signed_out" });
    expect(accountView(out, true)).toEqual({ kind: "waiting" });
    expect(accountView({ ...out, signing_in: true }, false)).toEqual({ kind: "waiting" });
  });

  it("shows who is signed in", () => {
    expect(accountView(signedIn, false)).toEqual({
      kind: "signed_in",
      name: "Alex",
      handle: "alex",
      avatar: "https://avatars.githubusercontent.com/u/1?v=4",
    });
    // The profile can't be read yet (offline): still signed in.
    expect(accountView({ ...signedIn, me: null }, false)).toEqual({
      kind: "signed_in",
      name: "Signed in",
      handle: null,
      avatar: null,
    });
  });

  it("loads avatars from GitHub's avatar host only", () => {
    expect(safeAvatar("https://avatars.githubusercontent.com/u/1?v=4")).toBe(
      "https://avatars.githubusercontent.com/u/1?v=4",
    );
    for (const bad of [
      "http://avatars.githubusercontent.com/u/1",
      "https://avatars.githubusercontent.com.example.test/u/1",
      "https://example.test/a.png",
      "javascript:alert(1)",
      "data:image/png;base64,AAAA",
      "not a url",
    ]) {
      expect(safeAvatar(bad)).toBeNull();
    }
    expect(safeAvatar(null)).toBeNull();
  });
});
