import { describe, expect, it } from "vitest";
import { readdirSync, readFileSync, statSync } from "node:fs";
import { dirname, join, relative } from "node:path";
import { fileURLToPath } from "node:url";

const SKILLS = join(dirname(fileURLToPath(import.meta.url)), "..", "plugin", "bmad", "skills");

/** The skills the Planner may follow (plugin/bmad/skills/ holds exactly these). */
const ALLOWLIST = ["bmad", "bmad-architecture", "bmad-prd", "bmad-review", "bmad-spec", "bmad-ux", "bmod-core-tools"];

/** Placeholder names written by scripts/patch-bmad.mjs; they are not skills. */
const PLACEHOLDERS = ["bmad-runtime", "bmad-root", "bmad-output"];

/**
 * Names that appear in the allowlisted text without being invoked: "next step" hints,
 * retired-skill lists, doc examples and URLs. Each needs a reason. Anything else that
 * looks like a BMAD skill name fails the test, so a new pin cannot quietly widen the
 * set of skills the text points at.
 */
const MENTIONED_NOT_INVOKED = {
  "bmad-code-org": "GitHub organisation in the install hints (the dcv2 line forbids installing)",
  "bmad-method": "documentation site name",
  "bmad-customize": "next-step hint; not available in a run, the Planner never customizes",
  "bmad-ticket": "next-step hint after a spec; the Planner writes the plan instead",
  "bmad-party-mode": "optional brainstorm helper; not available in a run",
  "bmad-advanced-elicitation": "optional helper; not available in a run",
  "bmad-forge-idea": "hand-off hint; not available in a run",
  "bmad-deep-recon": "hand-off hint; not available in a run",
  "bmad-build": "next-step hint; the Implementer builds",
  "bmad-brainstorming": "hand-off hint; not available in a run",
  "bmad-workflow-builder": "hint to a builder tool; not available in a run",
  "bmad-project-context": "hint to a project-context skill; not available in a run",
  "bmad-product-brief": "upstream input hint; not available in a run",
  "bmad-prfaq": "upstream input hint; not available in a run",
  "bmod-method": "module name in help text",
  "bmad-validate-prd": "retired name in a migration table",
  "bmad-shard-doc": "retired name (bmod-core-tools/retired.toml)",
  "bmad-init": "retired name (bmod-core-tools/retired.toml)",
  "bmad-index-docs": "retired name (bmod-core-tools/retired.toml)",
  "bmad-help-setup-modules": "retired name in help text",
  "bmad-editorial-review": "retired name in help text",
  "bmad-edit-prd": "retired name in a migration table",
  "bmad-distillator": "retired name (bmod-core-tools/retired.toml)",
  "bmad-create-prd": "retired name in a migration table",
};

function walk(dir) {
  return readdirSync(dir).flatMap((n) => {
    const p = join(dir, n);
    return statSync(p).isDirectory() ? walk(p) : [p];
  });
}

const files = walk(SKILLS).filter((f) => /\.(md|toml|html)$/.test(f));

describe("the BMAD skill allowlist", () => {
  it("has exactly the allowlisted skill folders", () => {
    expect(readdirSync(SKILLS).sort()).toEqual(ALLOWLIST);
  });

  it("every skill name in the text is allowlisted or in MENTIONED_NOT_INVOKED", () => {
    const known = new Set([...ALLOWLIST, ...PLACEHOLDERS, ...Object.keys(MENTIONED_NOT_INVOKED)]);
    const unknown = new Map();
    for (const f of files) {
      for (const m of readFileSync(f, "utf8").matchAll(/\b(?:bmad|bmod)-[a-z][a-z0-9-]*/g)) {
        if (!known.has(m[0])) unknown.set(m[0], relative(SKILLS, f));
      }
    }
    expect([...unknown.entries()]).toEqual([]);
  });

  it("MENTIONED_NOT_INVOKED has no stale entries and no allowlisted names", () => {
    const text = files.map((f) => readFileSync(f, "utf8")).join("\n");
    for (const name of Object.keys(MENTIONED_NOT_INVOKED)) {
      expect(ALLOWLIST).not.toContain(name);
      expect(text.includes(name), `${name} is not mentioned any more; remove it`).toBe(true);
    }
  });

  it("every skill: directive points at an allowlisted skill (comments with doc examples aside)", () => {
    const bad = [];
    for (const f of files) {
      for (const line of readFileSync(f, "utf8").split("\n")) {
        if (/^\s*#.*skill:acme-co/.test(line)) continue; // a documented example in a comment
        for (const m of line.matchAll(/"skill:([a-z0-9-]+)/g)) {
          if (!ALLOWLIST.includes(m[1])) bad.push(`${relative(SKILLS, f)}: skill:${m[1]}`);
        }
      }
    }
    expect(bad).toEqual([]);
  });

  it("bmad-review is invoked by the document skills' default customization", () => {
    const text = readFileSync(join(SKILLS, "bmad-prd", "customize.toml"), "utf8");
    expect(text).toMatch(/"skill:bmad-review lenses=structure,prose"/);
  });
});
