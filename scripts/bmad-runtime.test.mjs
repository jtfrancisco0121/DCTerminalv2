import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
const RUNTIME = join(ROOT, "plugin", "bmad", "runtime.mjs");
const ARCH_SKILL = join(ROOT, "plugin", "bmad", "skills", "bmad-architecture");

let base; // a temp folder; the run path inside it contains a space like "Application Support"
let run;
let bmad;
let outside;
let decoyCwd;

function runtime(args, { env = {}, cwd } = {}) {
  const r = spawnSync("node", [RUNTIME, ...args], {
    encoding: "utf8",
    cwd: cwd ?? base,
    env: { PATH: process.env.PATH, DCV2_RUN_DIR: run, ...env },
  });
  return { code: r.status, out: r.stdout, err: r.stderr };
}

beforeAll(() => {
  base = realpathSync(mkdtempSync(join(tmpdir(), "bmad-rt-")));
  run = join(base, "App Support", "run 1");
  bmad = join(run, "bmad");
  outside = join(base, "elsewhere");
  decoyCwd = join(base, "decoy repo");
  mkdirSync(join(bmad, "_bmad"), { recursive: true });
  mkdirSync(join(bmad, "output", "r1"), { recursive: true });
  mkdirSync(outside, { recursive: true });
  mkdirSync(join(decoyCwd, "_bmad"), { recursive: true });
  writeFileSync(join(bmad, "_bmad", "config.toml"), `[core]\nproject_name = "Toy"\noutput_folder = "${bmad}/output"\nactive_initiative = "r1"\n`);
  writeFileSync(join(decoyCwd, "_bmad", "config.toml"), `[core]\nproject_name = "DECOY"\n`);
});

afterAll(() => rmSync(base, { recursive: true, force: true }));

describe("allowed commands, in a run folder whose path has a space", () => {
  it("resolve_config reads <run>/bmad/_bmad/config.toml", () => {
    const r = runtime(["resolve_config", "--project-root", bmad, "--key", "core.project_name", "--key", "core.active_initiative"]);
    expect(r.code).toBe(0);
    expect(JSON.parse(r.out)).toEqual({ "core.project_name": "Toy", "core.active_initiative": "r1" });
  });

  it("adds --project-root when it is missing, and ignores a decoy _bmad in the cwd", () => {
    const r = runtime(["resolve_config", "--key", "core.project_name"], { cwd: decoyCwd });
    expect(r.code).toBe(0);
    expect(JSON.parse(r.out)).toEqual({ "core.project_name": "Toy" });
  });

  it("resolve_customization reads the skill's customize.toml", () => {
    const r = runtime(["resolve_customization", "--skill", ARCH_SKILL, "--key", "workflow"]);
    expect(r.code).toBe(0);
    expect(typeof JSON.parse(r.out)).toBe("object");
  });

  it("memlog init and append write inside bmad", () => {
    const ws = join(bmad, "output", "r1", "spec-toy");
    const a = runtime(["memlog", "init", "--workspace", ws, "--field", "topic=toy"]);
    expect(a.code, a.err + a.out).toBe(0);
    const b = runtime(["memlog", "append", "--workspace", ws, "--type", "note", "--text", "hello"]);
    expect(b.code, b.err + b.out).toBe(0);
    const log = readFileSync(join(ws, ".memlog.md"), "utf8");
    expect(log).toContain("hello");
  });

  it("memlog --path inside bmad works", () => {
    const p = join(bmad, "output", "r1", "other.memlog.md");
    expect(runtime(["memlog", "init", "--path", p]).code).toBe(0);
    expect(existsSync(p)).toBe(true);
  });

  it("knowledge reads a skills folder", () => {
    const r = runtime(["knowledge", "--root", join(ROOT, "plugin", "bmad", "skills")]);
    expect(r.code, r.err).toBe(0);
    expect(() => JSON.parse(r.out)).not.toThrow();
  });

  it("lint_spine runs on a spine fixture", () => {
    const ws = join(bmad, "output", "r1", "architecture-toy");
    mkdirSync(ws, { recursive: true });
    writeFileSync(
      join(ws, "architecture-toy.md"),
      "---\nname: toy\nstatus: draft\n---\n\n# Spine\n\n## Invariants & Rules\n\n### AD-1 - Layers\n\n- **Binds:** all\n- **Prevents:** drift\n- **Rule:** UI never calls the DB\n",
    );
    const r = runtime(["lint_spine", "--skill-root", ARCH_SKILL, "--workspace", ws]);
    expect(r.code, r.err).toBe(0);
    expect(JSON.parse(r.out).ok).toBe(true);
  });
});

describe("refusals", () => {
  it("refuses a command that is not on the list", () => {
    for (const cmd of ["render_skill", "tickets", "run_triggers", "setup", "git_evidence"]) {
      const r = runtime([cmd, "--project-root", bmad]);
      expect(r.code, cmd).toBe(2);
      expect(r.err).toMatch(/command not allowed/);
    }
  });

  it("refuses a missing command and a missing DCV2_RUN_DIR", () => {
    expect(runtime([]).code).toBe(2);
    const r = spawnSync("node", [RUNTIME, "resolve_config"], { encoding: "utf8", env: { PATH: process.env.PATH } });
    expect(r.status).toBe(1);
    expect(r.stderr).toMatch(/DCV2_RUN_DIR/);
  });

  it("refuses a --project-root elsewhere (also in --flag=value form)", () => {
    expect(runtime(["resolve_config", "--project-root", decoyCwd]).code).toBe(1);
    const r = runtime([`resolve_config`, `--project-root=${decoyCwd}`]);
    expect(r.code).toBe(1);
    expect(r.err).toMatch(/must be the run's bmad folder/);
    expect(runtime(["resolve_config", "--project-root", "relative/path"]).code).toBe(1);
  });

  it("refuses a --workspace or --path outside bmad, and does not write there", () => {
    for (const args of [
      ["memlog", "init", "--workspace", join(outside, "ws")],
      ["memlog", "init", "--path", join(outside, "x.memlog.md")],
      ["memlog", "append", "--workspace", run, "--type", "note", "--text", "x"], // the run folder itself is outside bmad
      ["memlog", "init", "--workspace", join(bmad, "..", "escape")],
      ["lint_spine", "--workspace", outside, "-o", join(outside, "o.json")],
    ]) {
      const r = runtime(args);
      expect(r.code, args.join(" ")).toBe(1);
      expect(r.err).toMatch(/outside the run's bmad folder/);
    }
    expect(existsSync(join(outside, "ws"))).toBe(false);
    expect(existsSync(join(outside, "x.memlog.md"))).toBe(false);
    expect(existsSync(join(run, "escape"))).toBe(false);
  });

  it("refuses a relative --workspace", () => {
    expect(runtime(["memlog", "init", "--workspace", "output/r1/x"], { cwd: bmad }).code).toBe(1);
  });

  it("refuses a symlinked folder inside bmad that points out", () => {
    symlinkSync(outside, join(bmad, "output", "linkdir"));
    const r = runtime(["memlog", "init", "--workspace", join(bmad, "output", "linkdir", "ws")]);
    expect(r.code).toBe(1);
    expect(r.err).toMatch(/outside the run's bmad folder/);
    expect(existsSync(join(outside, "ws"))).toBe(false);
  });

  it("refuses a symlinked file inside bmad that points out, and a dangling link", () => {
    const target = join(outside, "target.memlog.md");
    writeFileSync(target, "keep\n");
    symlinkSync(target, join(bmad, "output", "linkfile.memlog.md"));
    const r = runtime(["memlog", "append", "--path", join(bmad, "output", "linkfile.memlog.md"), "--type", "note", "--text", "x"]);
    expect(r.code).toBe(1);
    expect(readFileSync(target, "utf8")).toBe("keep\n");
    symlinkSync(join(outside, "missing-target"), join(bmad, "output", "dangling.memlog.md"));
    const d = runtime(["memlog", "init", "--path", join(bmad, "output", "dangling.memlog.md")]);
    expect(d.code).toBe(1);
    expect(existsSync(join(outside, "missing-target"))).toBe(false);
  });

  it("the runtime's own file system refuses writes outside bmad even when a path check was skipped", async () => {
    const { makeGuard, guardedFs } = await import(RUNTIME);
    const fs = guardedFs(await makeGuard(run));
    await expect(fs.writeText(join(outside, "w.txt"), "x")).rejects.toThrow(/outside the run's bmad folder/);
    await expect(fs.append(join(outside, "w.txt"), "x")).rejects.toThrow();
    await expect(fs.mkdir(join(outside, "d"))).rejects.toThrow();
    await expect(fs.delete(join(outside, "target.memlog.md"))).rejects.toThrow();
    await expect(fs.writeText(join(run, "plan.md"), "x")).rejects.toThrow();
    await fs.writeText(join(bmad, "ok.txt"), "x");
    expect(await fs.readText(join(outside, "target.memlog.md"))).toBe("keep\n"); // reads are free
  });
});
