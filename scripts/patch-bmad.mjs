/**
 * Vendor-time patch for the BMAD skills in plugin/bmad/skills/.
 *
 * Ported to plain JS from ADE's scripts/patchBmadSkills.ts
 * (https://github.com/alvin-reyes/better-agentic-ide, MIT, Copyright (c)
 * 2025-2026 Alvin Reyes). See plugin/bmad/NOTICE.md.
 *
 * It rewrites the upstream skill text so that:
 *   1. `uv run .../scripts/<x>.py` becomes `node "{bmad-runtime}" <x>` (only for the
 *      commands plugin/bmad/runtime.mjs allows);
 *   2. the runtime's project root is `{bmad-root}` (= <run>/bmad), never the repo;
 *   3. `{project-root}/_bmad/` becomes `{bmad-root}/_bmad/` and
 *      `{project-root}/_bmad-output` becomes `{bmad-output}`;
 *   4. each skill's line that defines `{project-root}` becomes one dcv2 definition;
 *   5. every placeholder path on a rewritten command line is double-quoted
 *      (run folders live under "Application Support").
 *
 * It is idempotent, and it throws on anything it does not recognise: upstream
 * churn must not ship unpatched.
 *
 * Usage: node scripts/patch-bmad.mjs <skills folder>
 */
import { readFileSync, writeFileSync, readdirSync } from "node:fs";
import { join, relative, sep } from "node:path";

/** The commands plugin/bmad/runtime.mjs allows (keep in sync with it). */
export const WRAPPER_COMMANDS = ["memlog", "resolve_config", "resolve_customization", "knowledge", "lint_spine"];

/** Scripts that ADE's runtime ports (ADE's PORTED set), by file name. */
const PORTED = new Set([
  "resolve_config.py", "resolve_customization.py", "config_utils.py",
  "tickets.py", "read_store.py", "render_skill.py", "memlog.py",
  "roster.py", "knowledge.py", "validate_manifests.py",
  "recon_kit.py", "init_skill.py", "brain.py", "process_template.py",
  "wake.py", "scan_scripts.py", "scan_paths.py", "resolve_party.py",
  "scan_legacy_module.py", "registry.py", "read_session_log.py",
  "pick_methods.py", "list_customizable_skills.py", "lint_spine.py",
  "resolve_personas.py", "run_triggers.py", "git_evidence.py",
]);

/**
 * Lines that legitimately keep `uv run`: scripts with external deps or dev
 * tooling, and prose. Anything else throws. In this vendored tree:
 *   word_metrics.py: bmad-review's editorial pass. The .py file is not vendored
 *     and the dcv2 line forbids `uv`; the skill falls back to estimates when the
 *     script cannot run (bmad-review/references/editorial-common.md).
 *   setup.py: BMAD's setup, which dcv2 never runs (the app makes the config).
 */
export const ALLOWED_UV = [
  "count_tokens.py", "prepass.py", "run_evals.py", "word_metrics.py",
  "<name>.py", "uv run pytest",
  "setup.py", "convert_cases.py", "aggregate_benchmark.py",
  "init-sanctum.py", "{script}.py", "<path>", "`uv run`",
];

/** The one dcv2 definition of the path placeholders. */
export const DCV2_MARKER = "dcv2 paths:";
export const DCV2_LINE =
  "dcv2 paths: `{project-root}` is the repo being planned (read-only; never write there); " +
  "`{bmad-root}` is `<run>/bmad`, the run's BMAD workspace; `{bmad-output}` is `<run>/bmad/output`; " +
  "`{bmad-runtime}` is `${CLAUDE_PLUGIN_ROOT}/bmad/runtime.mjs`; " +
  "`{skill-root}` is `${CLAUDE_PLUGIN_ROOT}/bmad/skills/<this skill>`. " +
  "These paths may contain spaces, so always put every placeholder path in double quotes in a shell command. " +
  "Never run `uv`, `npx` or any install or setup step.";

/** Known upstream phrasings of the `{project-root}` definition: [file, exact text]. */
const CONVENTION_PHRASES = [
  ["bmad/SKILL.md", "`{project-root}` is the nearest folder containing `_bmad/`, starting at the project working directory and moving up through its parents."],
  ["bmad-spec/SKILL.md", "`{project-root}` is the nearest folder containing `_bmad/`, starting at the working dir and moving up through its parents."],
  ["bmad-prd/SKILL.md", "`{project-root}` is the nearest folder containing `_bmad/`, starting at the project working dir and moving up through its parents."],
  ["bmad-review/SKILL.md", "`{project-root}` is the nearest folder containing `_bmad/`, starting at the project working directory and moving up through its parents."],
  ["bmad-architecture/SKILL.md", "`{project-root}` → the project working directory;"],
];
/** Known upstream prose that names a Python script path: [file, exact text, replacement]. */
const PROSE_PHRASES = [
  [
    "bmad-spec/SKILL.md",
    "`{project-root}/_bmad/scripts/memlog.py`, the same location as `resolve_customization.py`",
    'the `node "{bmad-runtime}" memlog` command, the same runtime as `resolve_customization`',
  ],
];
/** Skills whose upstream text has no definition line: the dcv2 line goes before this heading. */
const INSERT_BEFORE = [["bmad-ux/SKILL.md", "## On Activation"]];

/** A line that defines `{project-root}` (and is not already the dcv2 line). */
const DEFINES_PROJECT_ROOT = /`\{project-root\}`\s*(?:is\b|→|=|means\b)/;

const CALL = /(?<![a-z])uv run(?:\s+(?:--?[\w-]+|"[^"]*"|'[^']*'))*\s+"?(\{project-root\}\/_bmad\/(?:\w+\/)?scripts\/([\w.]+\.py)|\{skill-root\}\/scripts\/([\w.]+\.py))"?/g;

function rewriteCall(site, m) {
  const script = m[2] ?? m[3];
  if (ALLOWED_UV.some((a) => site.includes(a)) || ALLOWED_UV.includes(script)) return site;
  const name = script.replace(/\.py$/, "");
  if (!PORTED.has(script) || !WRAPPER_COMMANDS.includes(name)) {
    throw new Error(`no allowed runtime command for ${script}; call site cannot be patched: ${site.slice(0, 120)}`);
  }
  const skillRoot = m[3] ? ' --skill-root "{skill-root}"' : "";
  return `node "{bmad-runtime}" ${name}${skillRoot}`;
}

/** Double-quote bare `{placeholder}` values of path flags from `from` to the end of the code span. */
function quoteArgs(line) {
  const at = line.indexOf('node "{bmad-runtime}"');
  if (at < 0) return line;
  const head = line.slice(0, at);
  const tail = line.slice(at).replace(
    /(--(?:workspace|path|skill|skill-root|project-root|root)(?:\s+|=))(\{[^{}\s"`]+\})(?=[\s`.,;:)]|$)/g,
    '$1"$2"',
  );
  return head + tail;
}

/** Patch one file's text. `rel` is its path under the skills folder (forward slashes). */
export function patchText(rel, text) {
  const isMd = rel.endsWith(".md");
  let out = text;
  if (isMd) {
    // 1 + 2: runtime call sites.
    out = out
      .split("\n")
      .map((line) => {
        if (!line.includes("uv run")) return line;
        let hit = false;
        const next = line.replace(CALL, (site, _a, _b, _c) => {
          const m = new RegExp(CALL.source).exec(site);
          const r = rewriteCall(site, m);
          if (r !== site) hit = true;
          return r;
        });
        if (!hit) return line;
        return quoteArgs(next.replace(/--project-root\s+"?\{project-root\}"?/g, '--project-root "{bmad-root}"'));
      })
      .join("\n");
    // Fail loudly on any `uv run` that is neither rewritten nor allowed.
    for (const line of out.split("\n")) {
      if (!/\buv run\b/.test(line)) continue;
      if (ALLOWED_UV.some((a) => line.includes(a))) continue;
      throw new Error(`unpatched uv run in ${rel}: ${line.trim().slice(0, 120)}`);
    }
    // 4: the convention line.
    for (const [file, phrase] of CONVENTION_PHRASES) {
      if (rel === file && out.includes(phrase)) out = out.replace(phrase, DCV2_LINE);
    }
    for (const [file, phrase, replacement] of PROSE_PHRASES) {
      if (rel === file && out.includes(phrase)) out = out.replace(phrase, replacement);
    }
    for (const [file, heading] of INSERT_BEFORE) {
      if (rel === file && !out.includes(DCV2_MARKER)) {
        if (!out.includes(`\n${heading}\n`)) throw new Error(`${rel}: heading "${heading}" not found for the dcv2 path line`);
        out = out.replace(`\n${heading}\n`, `\n${DCV2_LINE}\n\n${heading}\n`);
      }
    }
    for (const line of out.split("\n")) {
      if (line.includes(DCV2_MARKER)) continue;
      if (DEFINES_PROJECT_ROOT.test(line)) {
        throw new Error(`unknown {project-root} definition in ${rel}: ${line.trim().slice(0, 140)}`);
      }
    }
  }
  if (isMd || rel.endsWith(".toml")) {
    // 3: runtime state lives under {bmad-root}; the output folder is {bmad-output}.
    out = out.replace(/\{project-root\}\/_bmad\//g, "{bmad-root}/_bmad/");
    out = out.replace(/\{project-root\}\/_bmad-output\b/g, "{bmad-output}");
  }
  return out;
}

function walk(dir) {
  return readdirSync(dir, { withFileTypes: true }).flatMap((e) =>
    e.isDirectory() ? walk(join(dir, e.name)) : [join(dir, e.name)],
  );
}

/** Patch every file under `root`; returns the files that changed. */
export function patchTree(root) {
  const changed = [];
  for (const p of walk(root)) {
    const rel = relative(root, p).split(sep).join("/");
    if (!rel.endsWith(".md") && !rel.endsWith(".toml")) continue;
    const body = readFileSync(p, "utf8");
    const next = patchText(rel, body);
    if (next !== body) {
      writeFileSync(p, next);
      changed.push(rel);
    }
  }
  return changed;
}

if (process.argv[1]?.endsWith("patch-bmad.mjs")) {
  try {
    const root = process.argv[2];
    if (!root) throw new Error("usage: node scripts/patch-bmad.mjs <skills folder>");
    const changed = patchTree(root);
    console.log(`patch-bmad: ${changed.length} file(s) changed`);
  } catch (err) {
    console.error(err instanceof Error ? err.message : err);
    process.exit(1);
  }
}
