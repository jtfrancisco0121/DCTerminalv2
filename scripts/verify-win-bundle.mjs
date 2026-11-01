// `npm run release:win` ends here: the NSIS bundle must carry dcv2-hook.exe next to the app
// (observe/hooks.rs looks for it beside the app exe). Tauri bundles every [[bin]] of the crate
// as it does on macOS, so no externalBin is used; this check fails loudly if the NSIS template
// did not take the second binary. Fallback if it ever fails: docs/launch.md "Windows install".
import { existsSync, readdirSync, readFileSync, statSync } from "node:fs";
import path from "node:path";
import process from "node:process";
import { pathToFileURL } from "node:url";

export const HOOK_EXE = "dcv2-hook.exe";
export const MISSING_MESSAGE = `${HOOK_EXE} is not in the NSIS bundle`;

/**
 * Whether the NSIS installer script (`installer.nsi`, left under target/release/nsis) installs
 * dcv2-hook.exe. Pure: returns `{ ok, message }`.
 */
export function checkInstallerScript(text) {
  const lines = String(text).split(/\r?\n/);
  const installs = lines.some(
    (line) => /^\s*File\b/i.test(line) && line.toLowerCase().includes(HOOK_EXE),
  );
  return installs ? { ok: true, message: `${HOOK_EXE} is in the NSIS bundle` } : { ok: false, message: MISSING_MESSAGE };
}

function findInstallerScripts(dir) {
  if (!existsSync(dir)) return [];
  const found = [];
  for (const name of readdirSync(dir)) {
    const full = path.join(dir, name);
    if (statSync(full).isDirectory()) found.push(...findInstallerScripts(full));
    else if (name.toLowerCase() === "installer.nsi") found.push(full);
  }
  return found;
}

export function verifyBundle(release = path.join("src-tauri", "target", "release")) {
  if (!existsSync(path.join(release, HOOK_EXE))) {
    return { ok: false, message: `${HOOK_EXE} was not built in ${release}` };
  }
  const scripts = findInstallerScripts(path.join(release, "nsis"));
  if (scripts.length === 0) {
    return { ok: false, message: `no NSIS installer script under ${path.join(release, "nsis")}` };
  }
  for (const script of scripts) {
    const result = checkInstallerScript(readFileSync(script, "utf8"));
    if (result.ok) return result;
  }
  return { ok: false, message: MISSING_MESSAGE };
}

const entry = process.argv[1];
if (entry && import.meta.url === pathToFileURL(path.resolve(entry)).href) {
  const result = verifyBundle();
  console.log(result.message);
  process.exit(result.ok ? 0 : 1);
}
