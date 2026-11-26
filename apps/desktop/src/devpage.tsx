// The dev page for the panel and the call bar (`npm run dev`, then open in a browser). Every
// state is reachable by URL so it can be looked at and screenshotted:
//
//   ?view=panel&state=home         one scenario of a surface, at its real size
//   ?view=bar&state=controlling
//   ?view=panel                    no state: the live mock, with its dev panel
//   ?view=both                     the panel and the bar side by side on the live mock
//   ?view=gallery&of=bar           every scenario of a surface in a grid
//   &frame=desktop                 over a busy pretend desktop, to judge legibility
//   &theme=light | &theme=dark     instead of following the system
//   &menu=1                        the panel's account menu or the bar's menu starts open
//   &window=1                      as the native window shows it: edge to edge, in a frame of
//                                  the height the window would be given (menus included)
//
// Development only: App.tsx imports this behind `import.meta.env.DEV`.

import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";

import type { Bridge } from "./bridge";
import { CallBarSurface } from "./CallBar";
import { observeHeight } from "./inWindow";
import { createMock } from "./mock";
import { PanelSurface } from "./Panel";
import { scenarioBridge } from "./scenarioBridge";
import { findScenario, scenariosOf, type Scenario, type Surface } from "./scenarios";

const VIEWS = ["panel", "bar", "both", "gallery"] as const;

function href(params: URLSearchParams, change: Record<string, string | null>): string {
  const next = new URLSearchParams(params);
  for (const [k, v] of Object.entries(change)) {
    if (v === null) {
      next.delete(k);
    } else {
      next.set(k, v);
    }
  }
  return `?${next.toString()}`;
}

/** `&window=1`: show surfaces as their native windows do. */
const asWindow = (): boolean => new URLSearchParams(window.location.search).get("window") === "1";
/** `&menu=1`: the panel's account menu, or the bar's menu, starts open in any state. */
const menuParam = (): boolean => new URLSearchParams(window.location.search).get("menu") === "1";

/**
 * A pretend native window: the `in-window` treatment, and the height `useWindowFit` would ask
 * the Rust side for, from the same `observeHeight`.
 */
function DevWindow({ surface, children }: { surface: Surface; children: ReactNode }) {
  const inner = useRef<HTMLDivElement>(null);
  const [height, setHeight] = useState<number | null>(null);
  useEffect(() => (inner.current === null ? undefined : observeHeight(inner.current, setHeight)), []);
  return (
    <div className={`dev-window in-window ${surface}-window`} style={height === null ? undefined : { height }}>
      <div ref={inner} className="window-root">
        {children}
      </div>
    </div>
  );
}

function SurfaceOf({ surface, bridge, scenario }: { surface: Surface; bridge: Bridge; scenario?: Scenario }) {
  const menuOpen = scenario?.menu === true || menuParam();
  const view =
    surface === "panel" ? (
      <PanelSurface bridge={bridge} settings={scenario?.settings === true} menuOpen={menuOpen} />
    ) : (
      <CallBarSurface bridge={bridge} menuOpen={menuOpen} />
    );
  return asWindow() ? <DevWindow surface={surface}>{view}</DevWindow> : view;
}

function Stage({ desktop, children }: { desktop: boolean; children: ReactNode }) {
  return <div className={`devstage${desktop ? " desktop" : ""}`}>{children}</div>;
}

function StateList({ params, surface }: { params: URLSearchParams; surface: Surface }) {
  return (
    <ul className="dev-states">
      {scenariosOf(surface).map((s) => (
        <li key={s.name}>
          <a href={href(params, { view: surface, state: s.name })}>{s.name}</a> <span className="muted">{s.title}</span>
        </li>
      ))}
    </ul>
  );
}

function Toolbar({ params }: { params: URLSearchParams }) {
  const link = (label: string, change: Record<string, string | null>, on: boolean) => (
    <a key={label} href={href(params, change)} aria-current={on ? "page" : undefined}>
      {label}
    </a>
  );
  const of = params.get("of") === "bar" ? "bar" : "panel";
  const theme = params.get("theme");
  const desktop = params.get("frame") === "desktop";
  return (
    <nav className="dev-toolbar" aria-label="Gallery">
      <span>
        {link("Panel", { of: "panel" }, of === "panel")}
        {link("Bar", { of: "bar" }, of === "bar")}
      </span>
      <span>
        {link("System", { theme: null }, theme === null)}
        {link("Light", { theme: "light" }, theme === "light")}
        {link("Dark", { theme: "dark" }, theme === "dark")}
      </span>
      <span>
        {link("Plain", { frame: null }, !desktop)}
        {link("Desktop", { frame: "desktop" }, desktop)}
      </span>
      <span>
        {link("Floating", { window: null }, !asWindow())}
        {link("In a window", { window: "1" }, asWindow())}
      </span>
    </nav>
  );
}

function Gallery({ params, base }: { params: URLSearchParams; base: Bridge }) {
  const of: Surface = params.get("of") === "bar" ? "bar" : "panel";
  const desktop = params.get("frame") === "desktop";
  const cells = useMemo(() => scenariosOf(of).map((s) => ({ scenario: s, bridge: scenarioBridge(s, base) })), [of, base]);
  return (
    <div className="dev-gallery-page">
      <Toolbar params={params} />
      <div className={`dev-gallery of-${of}`}>
        {cells.map(({ scenario, bridge }) => (
          <figure key={scenario.name} className="dev-cell">
            <figcaption>
              <a href={href(params, { view: of, state: scenario.name, of: null })}>
                <code>{scenario.name}</code>
              </a>
              <span className="muted">{scenario.title}</span>
            </figcaption>
            <Stage desktop={desktop}>
              <SurfaceOf surface={of} bridge={bridge} scenario={scenario} />
            </Stage>
          </figure>
        ))}
      </div>
    </div>
  );
}

export function DevPage({ params }: { params: URLSearchParams }) {
  const view = params.get("view") ?? "";
  const state = params.get("state");
  const desktop = params.get("frame") === "desktop";
  const [mock] = useState(() => createMock(params));

  useEffect(() => {
    const theme = params.get("theme");
    if (theme === "light" || theme === "dark") {
      document.documentElement.dataset.theme = theme;
    }
  }, [params]);

  if (view === "gallery") {
    return <Gallery params={params} base={mock.bridge} />;
  }

  const DevPanel = mock.DevPanel;
  if (view === "both") {
    return (
      <Stage desktop={desktop}>
        <div className="dev-both">
          <PanelSurface bridge={mock.bridge} />
          <CallBarSurface bridge={mock.bridge} />
        </div>
        {DevPanel !== null && <DevPanel />}
      </Stage>
    );
  }

  if (view !== "panel" && view !== "bar") {
    return (
      <Stage desktop={false}>
        <div className="dev-message">
          <p>
            Unknown view <code>{view}</code>. Use one of: {VIEWS.map((v) => <code key={v}>{v} </code>)}
          </p>
        </div>
      </Stage>
    );
  }

  if (state === null) {
    return (
      <Stage desktop={desktop}>
        <SurfaceOf surface={view} bridge={mock.bridge} />
        {DevPanel !== null && <DevPanel />}
      </Stage>
    );
  }

  const scenario = findScenario(view, state);
  if (scenario === undefined) {
    return (
      <Stage desktop={false}>
        <div className="dev-message" role="alert">
          <p>
            There's no {view} state called <code>{state}</code>. These are the ones there are:
          </p>
          <StateList params={params} surface={view} />
        </div>
      </Stage>
    );
  }
  return <ScenarioView key={`${view}-${state}`} scenario={scenario} base={mock.bridge} desktop={desktop} />;
}

function ScenarioView({ scenario, base, desktop }: { scenario: Scenario; base: Bridge; desktop: boolean }) {
  const [bridge] = useState(() => scenarioBridge(scenario, base));
  return (
    <Stage desktop={desktop}>
      <SurfaceOf surface={scenario.surface} bridge={bridge} scenario={scenario} />
    </Stage>
  );
}
