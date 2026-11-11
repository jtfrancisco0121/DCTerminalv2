// Test helper: a file of this repo as text, read with node's fs. The repo has no @types/node, so the module name goes
// through a variable (the same trick as StartScreen.width.test.ts). Never imported by app code.
type Fs = { readFileSync: (path: string, encoding: "utf8") => string };
const fsName = "node:fs";
const fs = (await import(/* @vite-ignore */ fsName)) as Fs;

/** A path relative to `src/` (for example `App.css`, `../src-tauri/capabilities/default.json`). */
export function readSource(relative: string): string {
  return fs.readFileSync(decodeURIComponent(new URL(relative, import.meta.url).pathname), "utf8");
}
