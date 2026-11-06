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

function mount() {
  ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
    <React.StrictMode>
      <App />
    </React.StrictMode>,
  );
}

// `npm run dev:fake` (QA): a faked backend is installed before the first render. Every `invoke` is lazy (none runs at
// module load), and a release build drops this branch: `MODE` is a literal there and `npm run check` scans dist/.
if (import.meta.env.MODE === "fake") {
  void import("./fake/install").then((fake) => {
    fake.installFakeBackend();
    mount();
  });
} else {
  mount();
}
