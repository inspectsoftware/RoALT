import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import "@fontsource-variable/geist";
import "@fontsource-variable/geist-mono";
import "./theme.css";
import App, { THEME_KEY } from "./App";

try {
  const t = localStorage.getItem(THEME_KEY);
  // The resolved theme (never "system").
  if (t === "light") document.documentElement.dataset.theme = t;

} catch {
  // No storage: the default (dark) applies until settings load.
}

// Plain-browser preview (`npm run dev` outside Tauri). Dead code in production builds.
if (import.meta.env.DEV && !("__TAURI_INTERNALS__" in window)) {
  await import("./dev/mockIpc");
}

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
