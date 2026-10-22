import React from "react";
import ReactDOM from "react-dom/client";
// Bundled fonts: UI (--font-ui) and code/terminal (--font-mono, xterm).
import "@fontsource-variable/inter";
import "@fontsource-variable/jetbrains-mono";
import App from "./App";
import { applyTheme, cachedTheme } from "./theme";

// Paint the last theme before settings.json loads. It reads storage inside
// try/catch and falls back to the default.
applyTheme(cachedTheme());

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
