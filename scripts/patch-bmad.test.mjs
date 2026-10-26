import { describe, expect, it } from "vitest";
import { readdirSync, readFileSync, statSync } from "node:fs";
import { dirname, join, relative } from "node:path";
import { fileURLToPath } from "node:url";
import { ALLOWED_UV, DCV2_LINE, DCV2_MARKER, patchText } from "./patch-bmad.mjs";

const SKILLS = join(dirname(fileURLToPath(import.meta.url)), "..", "plugin", "bmad", "skills");

function walk(dir) {
  return readdirSync(dir).flatMap((n) => {
    const p = join(dir, n);
    return statSync(p).isDirectory() ? walk(p) : [p];
  });
}

describe("patchText", () => {
  it("rewrites a {project-root} call site to the quoted wrapper call with {bmad-root}", () => {
    const line = "1. Resolve config: `uv run {project-root}/_bmad/scripts/resolve_config.py --project-root {project-root} --key core.output_folder`.";
    expect(patchText("bmad-spec/SKILL.md", line)).toBe(
      '1. Resolve config: `node "{bmad-runtime}" resolve_config --project-root "{bmad-root}" --key core.output_folder`.',
    );
  });

  it("rewrites a {skill-root} call site and quotes the placeholder paths", () => {
    const a = "Run `uv run {skill-root}/scripts/lint_spine.py --workspace {doc_workspace}` first.";
    expect(patchText("bmad-architecture/references/x.md", a)).toBe(
      'Run `node "{bmad-runtime}" lint_spine --skill-root "{skill-root}" --workspace "{doc_workspace}"` first.',
    );
    const b = "`uv run {project-root}/_bmad/scripts/resolve_customization.py --skill {skill-root} --project-root {project-root} --key workflow`";
    expect(patchText("bmad-prd/SKILL.md", b)).toBe(
      '`node "{bmad-runtime}" resolve_customization --skill "{skill-root}" --project-root "{bmad-root}" --key workflow`',
    );
    const c = '`uv run {project-root}/_bmad/scripts/memlog.py append --workspace {spec-folder} --type note --text "a b"`';
    expect(patchText("bmad-spec/SKILL.md", c)).toBe(
      '`node "{bmad-runtime}" memlog append --workspace "{spec-folder}" --type note --text "a b"`',
    );
  });

  it("rewrites runtime paths and the output folder", () => {
    const text = "see {project-root}/_bmad/custom/x.toml and {project-root}/_bmad-output and {project-root}/docs/a.md";
    expect(patchText("bmad-prd/customize.toml", text)).toBe(
      "see {bmad-root}/_bmad/custom/x.toml and {bmad-output} and {project-root}/docs/a.md",
    );
  });

  it("replaces the known {project-root} definition with the dcv2 line", () => {
    const text = "- `{skill-root}` is this skill's install dir; `{project-root}` is the nearest folder containing `_bmad/`, starting at the working dir and moving up through its parents.";
    const out = patchText("bmad-spec/SKILL.md", text);
    expect(out).toContain(DCV2_LINE);
    expect(out).not.toContain("nearest folder");
    expect(DCV2_LINE).toMatch(/double quotes/);
    expect(DCV2_LINE).toMatch(/Never run `uv`, `npx`/);
  });

  it("inserts the dcv2 line into bmad-ux, which has no definition line", () => {
    const out = patchText("bmad-ux/SKILL.md", "intro\n\n## On Activation\n\n1. go\n");
    expect(out).toContain(DCV2_MARKER);
    expect(out.indexOf(DCV2_MARKER)).toBeLessThan(out.indexOf("## On Activation"));
  });

  it("is idempotent", () => {
    const samples = [
      ["bmad-spec/SKILL.md", "- `{skill-root}` is this skill's install dir; `{project-root}` is the nearest folder containing `_bmad/`, starting at the working dir and moving up through its parents.\n1. `uv run {project-root}/_bmad/scripts/memlog.py init --workspace {spec-folder}`\n{project-root}/_bmad/x"],
      ["bmad-ux/SKILL.md", "intro\n\n## On Activation\n\n`uv run {skill-root}/scripts/lint_spine.py --workspace {w}`\n"],
    ];
    for (const [rel, text] of samples) {
      const once = patchText(rel, text);
      expect(patchText(rel, once)).toBe(once);
    }
  });

  it("keeps allowed uv run lines as they are", () => {
    const line = "via `uv run {skill-root}/scripts/word_metrics.py <path>`";
    expect(patchText("bmad-review/references/editorial-common.md", line)).toBe(line);
  });

  it("throws on an unknown uv run script", () => {
    expect(() => patchText("bmad-spec/SKILL.md", "`uv run {skill-root}/scripts/mystery.py --x`")).toThrow(/no allowed runtime command/);
  });

  it("throws on a ported script the wrapper does not allow", () => {
    expect(() => patchText("bmad-spec/SKILL.md", "`uv run {skill-root}/scripts/render_skill.py --x`")).toThrow(/no allowed runtime command/);
  });

  it("throws on a uv run it cannot recognise", () => {
    expect(() => patchText("bmad-spec/SKILL.md", "`uv run /opt/tool/other.py`")).toThrow(/unpatched uv run/);
  });

  it("throws on an unknown {project-root} definition", () => {
    expect(() => patchText("bmad-spec/SKILL.md", "`{project-root}` is the folder with a package.json.")).toThrow(/unknown \{project-root\} definition/);
  });
});

describe("the vendored tree", () => {
  const files = walk(SKILLS);
  const text = files.filter((f) => f.endsWith(".md") || f.endsWith(".toml"));

  it("holds exactly the 7 allowlisted skills", () => {
    expect(readdirSync(SKILLS).sort()).toEqual(
      ["bmad", "bmad-architecture", "bmad-prd", "bmad-review", "bmad-spec", "bmad-ux", "bmod-core-tools"],
    );
  });

  it("has no Python file and no tests folder", () => {
    expect(files.filter((f) => f.endsWith(".py"))).toEqual([]);
    expect(files.filter((f) => /\/tests\//.test(f))).toEqual([]);
  });

  it("is already patched: patching again changes nothing", () => {
    for (const f of text) {
      const rel = relative(SKILLS, f);
      const body = readFileSync(f, "utf8");
      expect(patchText(rel, body), rel).toBe(body);
    }
  });

  it("keeps no uv run outside ALLOWED_UV", () => {
    for (const f of text.filter((p) => p.endsWith(".md"))) {
      for (const line of readFileSync(f, "utf8").split("\n")) {
        if (!/\buv run\b/.test(line)) continue;
        expect(ALLOWED_UV.some((a) => line.includes(a)), `${f}: ${line}`).toBe(true);
      }
    }
  });

  it("keeps word_metrics.py as the only allowed uv run call to a vendored-away script, and setup.py as text", () => {
    const left = new Set();
    for (const f of text.filter((p) => p.endsWith(".md"))) {
      for (const m of readFileSync(f, "utf8").matchAll(/uv run[^\n]*?([\w-]+\.py)/g)) left.add(m[1]);
    }
    expect([...left].sort()).toEqual(["setup.py", "word_metrics.py"]);
  });

  it("has no {project-root}/_bmad path and no ade-runtime.mjs call site", () => {
    for (const f of text) {
      const body = readFileSync(f, "utf8");
      expect(body, f).not.toMatch(/\{project-root\}\/_bmad(\/|-output)/);
      expect(body, f).not.toMatch(/ade-runtime\.mjs/);
    }
  });

  it("quotes every placeholder on every runtime call site", () => {
    let calls = 0;
    for (const f of text.filter((p) => p.endsWith(".md"))) {
      for (const line of readFileSync(f, "utf8").split("\n")) {
        for (const m of line.matchAll(/node "\{bmad-runtime\}"[^`\n]*/g)) {
          calls += 1;
          expect(m[0], f).not.toMatch(/--(?:workspace|path|skill|skill-root|project-root|root)[ =]\{/);
          expect(m[0], f).not.toMatch(/\{project-root\}/);
        }
      }
    }
    expect(calls).toBeGreaterThan(15);
  });

  it("defines the path placeholders once per skill that mentions them", () => {
    for (const skill of ["bmad", "bmad-spec", "bmad-prd", "bmad-architecture", "bmad-ux", "bmad-review"]) {
      expect(readFileSync(join(SKILLS, skill, "SKILL.md"), "utf8"), skill).toContain(DCV2_LINE);
    }
  });
});
