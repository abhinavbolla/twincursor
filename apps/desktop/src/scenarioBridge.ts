// A pretend Rust side that starts from a scenario (scenarios.ts) and answers the panel's and the
// bar's flows plausibly, so a canned state can also be clicked through. Development only: the dev
// page imports it, and only `import.meta.env.DEV` code imports the dev page.

import type { Bridge } from "./bridge";
import type { CallState } from "./call";
import type { HomeView, Person } from "./home";
import type { EngineCommand, EngineOutput } from "./ipc";
import { MAC, CANT_SHARE, type PlatformInfo } from "./platform";
import { ALEX, asGuest, asSharer, boot, guestIdentity, LINK, out, peer, type Scenario } from "./scenarios";
import { replay } from "./useSurface";

const NOT_INVITED = "This server is invite-only. Ask whoever runs it to add your GitHub handle.";

/**
 * A bridge over `base` (the mock, for everything else) whose home view and call
 * come from the scenario and change as the person clicks.
 */
export function scenarioBridge(scenario: Scenario, base: Bridge, platform: PlatformInfo = MAC): Bridge {
  let home: HomeView = scenario.home;
  let outputs: EngineOutput[] = [...scenario.outputs];
  let call: CallState = replay(outputs);
  const homeListeners = new Set<(h: HomeView) => void>();
  const outputListeners = new Set<(o: EngineOutput) => void>();

  const setHome = (change: Partial<HomeView>): HomeView => {
    home = { ...home, ...change };
    homeListeners.forEach((f) => f(home));
    return home;
  };
  const emit = (...more: EngineOutput[]) => {
    for (const output of more) {
      outputs = [...outputs, output];
      call = replay([output], call);
      outputListeners.forEach((f) => f(output));
    }
  };
  const later = (ms: number, f: () => void) => void window.setTimeout(f, ms);
  const me = (): Person => home.account.me ?? ALEX;
  const control = (state: "none" | "asked" | "controlling", by: { identity: string; name: string } | null, trusted = call.control.trusted) =>
    emit(out.control(state, by, trusted));

  const ok = (): Promise<HomeView> => Promise.resolve(home);
  const refuse = (sentence: string): Promise<HomeView> => Promise.reject(new Error(sentence));

  const endCall = () => {
    if (call.phase === "in_call") {
      emit(out.connection("disconnected"), out.exited(0));
    }
    outputs = [];
    call = replay([]);
    return setHome({ stage: { kind: "idle" }, knocks: [], members: [] });
  };

  const startJoined = (host: Person) => {
    setHome({ stage: { kind: "joined", host }, rings: home.rings.filter((r) => r.host.id !== host.id) });
    emit(...boot("join"), out.participants([asSharer(host)]), out.stats("join", "h264"));
  };

  const command = (cmd: EngineCommand) => {
    const asked = call.asked;
    const myIdentity = guestIdentity(me());
    switch (cmd.cmd) {
      case "mute":
      case "unmute":
        emit(out.voice(cmd.cmd === "mute"));
        return;
      case "leave":
        endCall();
        return;
      case "allow_control":
        if (asked !== null) {
          control("controlling", { identity: asked.identity, name: asked.name });
        }
        return;
      case "deny_control":
        control("none", null);
        return;
      case "revoke_control":
        control("none", null, []);
        return;
      case "trust_control":
        return;
      case "request_control":
        control("asked", { identity: myIdentity, name: me().name });
        later(1500, () => {
          if (call.control.state === "asked") {
            control("controlling", { identity: myIdentity, name: me().name });
          }
        });
        return;
      case "release_control":
        control("none", null);
        return;
      case "set_mic":
      case "set_speaker":
        return;
    }
  };

  return {
    ...base,
    homeState: () => Promise.resolve(home),
    callSnapshot: () => Promise.resolve([...outputs]),
    onHome: (f) => {
      homeListeners.add(f);
      return () => homeListeners.delete(f);
    },
    onOutput: (f) => {
      outputListeners.add(f);
      return () => outputListeners.delete(f);
    },
    send: (cmd) => {
      if (call.phase !== "in_call") {
        return Promise.reject(new Error("No call is running."));
      }
      command(cmd);
      return Promise.resolve();
    },
    signIn: () => {
      setHome({ account: { ...home.account, signing_in: true } });
      return new Promise((resolve) =>
        later(2000, () => {
          const account = { ...home.account, configured: true, signed_in: true, me: ALEX, signing_in: false };
          setHome({ account });
          resolve(account);
        }),
      );
    },
    cancelSignIn: () => {
      setHome({ account: { ...home.account, signing_in: false } });
      return Promise.resolve();
    },
    signOut: () => {
      const account = { ...home.account, signed_in: false, me: null, signing_in: false };
      setHome({ account, contacts: [] });
      return Promise.resolve(account);
    },
    platformInfo: () => Promise.resolve(platform),
    share: () => {
      if (!platform.can_share) {
        return refuse(CANT_SHARE);
      }
      if (!home.account.can_host) {
        return refuse(NOT_INVITED);
      }
      if (home.stage.kind !== "idle") {
        return refuse("You're already in a call. Leave it first.");
      }
      if (home.needs === "screen_recording") {
        return refuse("TwinCursor needs Screen Recording to share your screen. Turn it on in System Settings, then try again.");
      }
      setHome({ stage: { kind: "starting" }, error: null });
      return new Promise((resolve) =>
        later(900, () => {
          emit(...boot("share"));
          resolve(setHome({ stage: { kind: "hosting", call_id: "c1", link: LINK }, notice: "The call link is on your clipboard." }));
        }),
      );
    },
    joinFromClipboard: () => {
      const host = home.contacts[0];
      if (home.stage.kind !== "idle" || host === undefined) {
        return refuse(host === undefined ? "There's no call link to join. Copy the link first." : "You're already in a call. Leave it first.");
      }
      later(2500, () => {
        if (home.stage.kind === "knocking") {
          startJoined(host);
        }
      });
      return Promise.resolve(setHome({ stage: { kind: "knocking", host }, error: null }));
    },
    cancelKnock: () => (home.stage.kind === "knocking" ? Promise.resolve(setHome({ stage: { kind: "idle" } })) : refuse("You aren't waiting to join a call.")),
    answerKnock: (userId, letIn, withControl) => {
      const knock = home.knocks.find((k) => k.user_id === userId);
      if (knock === undefined) {
        return refuse("That person isn't waiting to join any more.");
      }
      if (letIn && !knock.armed) {
        return refuse("Too soon. Try again.");
      }
      const knocks = home.knocks.filter((k) => k.user_id !== userId);
      if (!letIn) {
        return Promise.resolve(setHome({ knocks }));
      }
      const person: Person = { id: knock.user_id, name: knock.name, handle: knock.handle, avatar_url: knock.avatar_url };
      const guest = asGuest(person);
      setHome({
        knocks,
        members: [...home.members, { user_id: person.id, name: person.name, handle: person.handle, avatar_url: person.avatar_url, state: "admitted" }],
      });
      later(600, () => {
        emit(out.participants([...call.participants, guest]));
        if (withControl) {
          control(call.control.state, call.control.by, [...call.control.trusted, peer(guest)]);
        }
      });
      return ok();
    },
    giveControl: (identity) => {
      const guest = call.participants.find((p) => p.identity === identity);
      if (guest === undefined) {
        return refuse("That person isn't in the call.");
      }
      if (home.needs === "accessibility") {
        return refuse("TwinCursor needs Accessibility to give control. Turn it on in System Settings, then try again.");
      }
      control(call.control.state, call.control.by, [...call.control.trusted.filter((t) => t.identity !== identity), peer(guest)]);
      return Promise.resolve(setHome({ owed: home.owed?.identity === identity ? null : home.owed }));
    },
    giveOwedControl: () => {
      const owed = home.owed;
      if (owed === null) {
        return refuse("Nobody is waiting to be given control.");
      }
      if (!owed.armed) {
        return refuse("Too soon. Try again.");
      }
      const guest = call.participants.find((p) => p.identity === owed.identity);
      if (guest !== undefined) {
        control(call.control.state, call.control.by, [...call.control.trusted.filter((t) => t.identity !== owed.identity), peer(guest)]);
      }
      return Promise.resolve(setHome({ owed: null }));
    },
    resetPermission: (which) => {
      if (home.needs !== which) {
        return refuse("macOS isn't refusing that permission now.");
      }
      // Pretend the reset and macOS's prompt went through: still needed, not stuck.
      return Promise.resolve(setHome({ stuck: false }));
    },
    ring: (userId) => {
      if (!platform.can_share) {
        return refuse(CANT_SHARE);
      }
      if (!home.account.can_host) {
        return refuse(NOT_INVITED);
      }
      const contact = home.contacts.find((c) => c.id === userId);
      if (contact === undefined) {
        return refuse("You can only ring someone who has you as a contact too.");
      }
      const ringing = () =>
        setHome({
          members: [
            ...home.members.filter((m) => m.user_id !== userId),
            { user_id: contact.id, name: contact.name, handle: contact.handle, avatar_url: contact.avatar_url, state: "ringing" },
          ],
        });
      if (home.stage.kind === "hosting") {
        return Promise.resolve(ringing());
      }
      if (home.stage.kind !== "idle") {
        return refuse("You're already in a call. Leave it first.");
      }
      setHome({ stage: { kind: "starting" } });
      return new Promise((resolve) =>
        later(900, () => {
          emit(...boot("share"));
          setHome({ stage: { kind: "hosting", call_id: "c1", link: LINK } });
          resolve(ringing());
        }),
      );
    },
    answerRing: (callId, accept) => {
      const ring = home.rings.find((r) => r.call_id === callId);
      if (ring === undefined) {
        return refuse("That call isn't ringing you any more.");
      }
      if (!accept) {
        return Promise.resolve(setHome({ rings: home.rings.filter((r) => r.call_id !== callId) }));
      }
      if (home.stage.kind !== "idle") {
        endCall();
      }
      startJoined(ring.host);
      return ok();
    },
    askToHost: () => Promise.resolve(setHome({ account: { ...home.account, asked_to_host: true } })),
    end: () => {
      setHome({ stage: { kind: "ending" } });
      return new Promise((resolve) => later(400, () => resolve(endCall())));
    },
  };
}
