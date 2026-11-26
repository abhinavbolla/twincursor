import { lazy, Suspense, useEffect, useState } from "react";

import type { LoadedBridge } from "./bridge";
import { inTauri, loadBridge } from "./bridge";
import { CallBarSurface } from "./CallBar";
import { useWindowFit, viewOf, type WindowView } from "./inWindow";
import { PanelSurface } from "./Panel";

/** The dev page for the panel and the bar: in a browser under `vite` only. */
const DevPage = import.meta.env.DEV ? lazy(() => import("./devpage").then((m) => ({ default: m.DevPage }))) : null;

/** One surface alone, as its own native window shows it, fitted to it. */
function WindowSurface({ view }: { view: WindowView }) {
  const [loaded, setLoaded] = useState<LoadedBridge | null | undefined>(undefined);
  const root = useWindowFit(view);
  useEffect(() => {
    loadBridge().then(setLoaded, () => setLoaded(null));
  }, []);
  return (
    <div ref={root} className="window-root">
      {loaded != null &&
        (view === "bar" ? <CallBarSurface bridge={loaded.bridge} /> : <PanelSurface bridge={loaded.bridge} />)}
    </div>
  );
}

export function App() {
  const params = new URLSearchParams(window.location.search);
  if (inTauri()) {
    return <WindowSurface view={viewOf(params)} />;
  }
  if (DevPage !== null) {
    // Without a view, the dev page shows the panel on the live mock.
    if (params.get("view") === null) {
      params.set("view", "panel");
    }
    return (
      <Suspense fallback={null}>
        <DevPage params={params} />
      </Suspense>
    );
  }
  return (
    <main>
      <h1>TwinCursor</h1>
      <p>This page runs inside the TwinCursor app.</p>
    </main>
  );
}
