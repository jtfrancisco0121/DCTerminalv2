import { mkdtempSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { spawnSync } from "node:child_process";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, describe, expect, it } from "vitest";
import { MARKER, filesWithMarker } from "./no-fake-in-dist.mjs";

const script = path.join(path.dirname(fileURLToPath(import.meta.url)), "no-fake-in-dist.mjs");
let dir;
afterEach(() => dir && rmSync(dir, { recursive: true, force: true }));

function dist(files) {
  dir = mkdtempSync(path.join(os.tmpdir(), "dcv2-dist-"));
  for (const [name, text] of Object.entries(files)) {
    mkdirSync(path.dirname(path.join(dir, name)), { recursive: true });
    writeFileSync(path.join(dir, name), text);
  }
  return dir;
}

describe("the dist/ scan for the faked backend", () => {
  it("finds the marker in a nested file and nowhere else", () => {
    const root = dist({ "index.html": "<html></html>", "assets/app.js": `x("${MARKER}")`, "assets/other.js": "y()" });
    expect(filesWithMarker(root).map((f) => path.relative(root, f))).toEqual([path.join("assets", "app.js")]);
  });

  it("exits 1 naming the file when the marker is there, 0 on a clean build, 1 with no dist/", () => {
    const bad = spawnSync(process.execPath, [script, dist({ "a.js": MARKER })], { encoding: "utf8" });
    expect(bad.status).toBe(1);
    expect(bad.stderr).toContain("a.js");
    const clean = spawnSync(process.execPath, [script, dist({ "a.js": "ok" })], { encoding: "utf8" });
    expect(clean.status).toBe(0);
    expect(spawnSync(process.execPath, [script, path.join(dir, "missing")], { encoding: "utf8" }).status).toBe(1);
  });
});
