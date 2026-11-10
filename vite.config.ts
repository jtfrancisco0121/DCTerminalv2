import { defineConfig, searchForWorkspaceRoot } from "vite";
import react from "@vitejs/plugin-react";
// @ts-expect-error type error without @types/node package
import fs from "node:fs";
// @ts-expect-error type error without @types/node package
import path from "node:path";
// @ts-expect-error type error without @types/node package
import process from "node:process";
const host = process.env.TAURI_DEV_HOST;

/**
 * The folders `npm run dev:fake` may serve. A QA worktree can reach its packages through a symlinked node_modules, and Vite then
 * answered 403 for /@fs/<real node_modules>/@fontsource-variable/inter/files/inter-latin-wght-normal.woff2 on every load. Setting
 * `allow` turns off Vite's automatic workspace root, so it is listed again; the real node_modules folder is added by its exact
 * realpath (never a wildcard root or the home folder).
 */
export function fakeFsAllow(cwd: string): string[] {
  const allow = [searchForWorkspaceRoot(cwd)];
  try {
    const modules = fs.realpathSync(path.join(cwd, "node_modules"));
    if (!allow.includes(modules)) allow.push(modules);
  } catch {
    // no node_modules next to the project: nothing to add
  }
  return allow;
}

// https://vite.dev/config/
export default defineConfig(({ mode }) => {
  // `npm run dev:fake` (mode "fake"): the UI in a plain browser on a faked backend, for QA. Parallel QA servers and the
  // user's `npm run dev:app` must not collide, so the port comes from PORT and the fixed HMR port 1421 is not used.
  const fake = mode === "fake";
  const port =
    fake && /^\d{2,5}$/.test(process.env.PORT ?? "")
      ? Number(process.env.PORT)
      : 1420;
  return {
    plugins: [react()],

    // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
    //
    // 1. prevent Vite from obscuring rust errors
    clearScreen: false,
    // 2. tauri expects a fixed port, fail if that port is not available
    server: {
      port,
      strictPort: true,
      ...(fake ? { fs: { allow: fakeFsAllow(process.cwd()) } } : {}),
      // Listen on localhost so the Tauri webview and HMR websocket are reliable on Windows.
      host: host || "127.0.0.1",
      hmr: fake
        ? false
        : {
            protocol: "ws",
            host: host || "127.0.0.1",
            port: 1421,
            clientPort: 1421,
          },
      watch: {
        // 3. tell Vite to ignore watching `src-tauri`
        ignored: ["**/src-tauri/**"],
      },
    },
  };
});
