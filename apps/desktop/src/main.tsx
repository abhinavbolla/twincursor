import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import { App } from "./App";
import { inTauri } from "./bridge";
import { markInWindow, viewOf } from "./inWindow";
import "./styles.css";

const root = document.getElementById("root");
if (root === null) {
  throw new Error("missing #root");
}
if (inTauri()) {
  markInWindow(viewOf(new URLSearchParams(window.location.search)));
}
createRoot(root).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
