// Gate step after `npm run build`: the faked backend (src/fake, `npm run dev:fake`) must never be in a release build.
// Fails when any file under dist/ contains the fake's marker string.
import { existsSync, readdirSync, readFileSync, statSync } from "node:fs";
import path from "node:path";
import process from "node:process";
import { pathToFileURL } from "node:url";

export const MARKER = "DCV2_FAKE_BACKEND_MARKER";

/** The files under `dir` (recursively) whose bytes contain the marker. */
export function filesWithMarker(dir, marker = MARKER) {
  const found = [];
  for (const name of readdirSync(dir)) {
    const file = path.join(dir, name);
    if (statSync(file).isDirectory()) found.push(...filesWithMarker(file, marker));
    else if (readFileSync(file).includes(marker)) found.push(file);
  }
  return found;
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  const dist = process.argv[2] ?? "dist";
  if (!existsSync(dist)) {
    console.error(`${dist}/ does not exist: run npm run build first`);
    process.exit(1);
  }
  const hits = filesWithMarker(dist);
  if (hits.length > 0) {
    console.error(`the faked backend marker ${MARKER} is in the build: ${hits.join(", ")}`);
    process.exit(1);
  }
  console.log(`no faked backend in ${dist}/`);
}
