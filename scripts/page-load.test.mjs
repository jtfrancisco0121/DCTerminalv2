import { describe, expect, it } from "vitest";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import config, { fakeFsAllow } from "../vite.config.ts";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");

describe("a clean dev:fake page load", () => {
  it("links an icon that exists under public/, so the page never asks for /favicon.ico", () => {
    const html = readFileSync(join(ROOT, "index.html"), "utf8");
    const href = /<link[^>]+rel="icon"[^>]+href="([^"]+)"/.exec(html)?.[1];
    expect(href, "index.html has an icon link").toBeTruthy();
    expect(href.startsWith("/")).toBe(true);
    expect(existsSync(join(ROOT, "public", href.slice(1)))).toBe(true);
  });

  // The 403 seen on every dev:fake load from a QA worktree whose node_modules is a symlink: Vite refused
  // /@fs/<real node_modules>/@fontsource-variable/inter/files/inter-latin-wght-normal.woff2.
  it("allows the real node_modules folder of a symlinked node_modules, and the workspace root", () => {
    const base = realpathSync(mkdtempSync(join(tmpdir(), "dcv2-fsallow-")));
    const project = join(base, "wt");
    const modules = join(base, "shared", "node_modules");
    mkdirSync(project, { recursive: true });
    mkdirSync(modules, { recursive: true });
    writeFileSync(join(project, "package.json"), "{}");
    symlinkSync(modules, join(project, "node_modules"));
    const allow = fakeFsAllow(project);
    expect(allow).toContain(project);
    expect(allow).toContain(modules);
    expect(allow).not.toContain("/");
    expect(allow.every((p) => p.length > 1 && !p.includes("*"))).toBe(true);
  });

  it("sets server.fs.allow in fake mode only", () => {
    const fake = config({ mode: "fake", command: "serve" });
    const dev = config({ mode: "development", command: "serve" });
    expect(fake.server.fs.allow.length).toBeGreaterThan(0);
    expect(dev.server.fs).toBeUndefined();
  });
});
