// The two things both surfaces read: the home view and the call. Each window keeps its own copy,
// fresh from the Rust side, so the panel and the bar can live in separate native windows.

import { useEffect, useReducer, useState } from "react";

import type { Bridge } from "./bridge";
import { callReducer, initialCall, type CallState } from "./call";
import type { HomeView } from "./home";
import type { EngineOutput } from "./ipc";
import { MAC, type PlatformInfo } from "./platform";

/** The home view: `homeState()` once, then every `home` event. Null until the first arrives. */
export function useHome(bridge: Bridge): HomeView | null {
  const [home, setHome] = useState<HomeView | null>(null);
  useEffect(() => {
    let live = true;
    let heard = false;
    const stop = bridge.onHome((h) => {
      heard = true;
      setHome(h);
    });
    bridge.homeState().then(
      (h) => {
        // An event that came in meanwhile is newer than this answer.
        if (live && !heard) {
          setHome(h);
        }
      },
      () => undefined,
    );
    return () => {
      live = false;
      stop();
    };
  }, [bridge]);
  return home;
}

/**
 * What the platform can do, asked once. Null until it answers; a bridge that can't answer is
 * taken for a Mac (what the app was before it asked).
 */
export function usePlatform(bridge: Bridge): PlatformInfo | null {
  const [platform, setPlatform] = useState<PlatformInfo | null>(null);
  useEffect(() => {
    let live = true;
    bridge.platformInfo().then(
      (p) => live && setPlatform(p),
      () => live && setPlatform(MAC),
    );
    return () => {
      live = false;
    };
  }, [bridge]);
  return platform;
}

type Action = { type: "replace"; state: CallState } | { type: "output"; output: EngineOutput };

function reduce(state: CallState, action: Action): CallState {
  return action.type === "replace" ? action.state : callReducer(state, action);
}

export const replay = (outputs: readonly EngineOutput[], from: CallState = initialCall): CallState =>
  outputs.reduce((s, output) => callReducer(s, { type: "output", output }), from);

/**
 * The call: the snapshot replayed through the reducer, then every engine output. Output heard
 * while the snapshot was on its way is applied again on top of it; what the engine sends sets
 * state (connection, people, control, voice, numbers), so hearing one twice changes nothing.
 */
export function useCall(bridge: Bridge): CallState {
  const [call, dispatch] = useReducer(reduce, initialCall);
  useEffect(() => {
    let live = true;
    let early: EngineOutput[] | null = [];
    const stop = bridge.onOutput((output) => {
      if (early !== null) {
        early.push(output);
      }
      dispatch({ type: "output", output });
    });
    bridge.callSnapshot().then(
      (snapshot) => {
        if (live) {
          dispatch({ type: "replace", state: replay([...snapshot, ...(early ?? [])]) });
        }
        early = null;
      },
      () => {
        early = null;
      },
    );
    return () => {
      live = false;
      stop();
    };
  }, [bridge]);
  return call;
}
