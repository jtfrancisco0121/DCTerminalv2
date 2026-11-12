import { describe, expect, it } from "vitest";
import app from "../../fixtures/fake/app.json";
import clickup from "../../fixtures/fake/clickup.json";
import notFaked from "../../fixtures/fake/not-faked.json";
import run from "../../fixtures/fake/run.json";
import runs from "../../fixtures/fake/runs.json";
import settings from "../../fixtures/fake/settings.json";
import { BEHAVIOUR_ONLY } from "./install";

// Every command the UI invokes is either faked (a fixture, or behaviour alone: `BEHAVIOUR_ONLY` of install.ts) or listed
// with a reason in fixtures/fake/not-faked.json.
// Adding an `invoke("new_command")` in src/ without one of the two fails here, naming the command.
const sources = import.meta.glob("/src/**/*.{ts,tsx}", { query: "?raw", import: "default", eager: true }) as Record<
  string,
  string
>;

/** `const NAME = "command"` bindings of one file (also `export const`), so `invoke(NAME)` resolves. */
function constantsOf(text: string): Map<string, string> {
  const bound = new Map<string, string>();
  for (const m of text.matchAll(/\bconst\s+([A-Za-z_$][\w$]*)\s*(?::\s*[\w\s"'|]+)?=\s*(?:"([A-Za-z0-9_:|]+)"|'([A-Za-z0-9_:|]+)'|`([A-Za-z0-9_:|]+)`)\s*(?:as const)?\s*[;\n]/g)) {
    bound.set(m[1]!, (m[2] ?? m[3] ?? m[4])!);
  }
  return bound;
}

/**
 * The calls of `invoke` in a file: each has the first argument's text. A call is `invoke(` or `invoke<...>(` (type arguments
 * may nest); a declaration (`invoke(command: string`) or a mention in a comment is not one.
 */
function invokeCalls(text: string): string[] {
  const calls: string[] = [];
  for (const m of text.matchAll(/\binvoke\b/g)) {
    let i = m.index! + m[0].length;
    while (/\s/.test(text[i] ?? "")) i++;
    if (text[i] === "<") {
      let depth = 0;
      for (; i < text.length && i < m.index! + 400; i++) {
        if (text[i] === "<") depth++;
        else if (text[i] === ">" && text[i - 1] !== "=" && --depth === 0) {
          i++;
          break;
        }
      }
      if (depth !== 0) continue;
      while (/\s/.test(text[i] ?? "")) i++;
    }
    if (text[i] !== "(") continue;
    const rest = text.slice(i + 1);
    const end = rest.search(/[,)]/);
    calls.push(rest.slice(0, end < 0 ? 200 : Math.min(end, 200) + 1));
  }
  return calls;
}

/** What each `invoke(` call asks for: resolved commands, and the calls whose command cannot be read from the source. */
export function resolveInvokes(text: string): { commands: string[]; unresolved: string[] } {
  const bound = constantsOf(text);
  const commands: string[] = [];
  const unresolved: string[] = [];
  for (const call of invokeCalls(text)) {
    const arg = call.trim();
    const quoted = /^(?:"([A-Za-z0-9_:|]+)"|'([A-Za-z0-9_:|]+)'|`([A-Za-z0-9_:|]+)`)[,)]?$/.exec(arg);
    const identifier = /^([A-Za-z_$][\w$]*)[,)]?$/.exec(arg);
    if (quoted) commands.push((quoted[1] ?? quoted[2] ?? quoted[3])!);
    else if (identifier && bound.has(identifier[1]!)) commands.push(bound.get(identifier[1]!)!);
    else if (/^[`"']/.test(arg) || identifier) unresolved.push(arg.replace(/[,)]$/, ""));
    // anything else (a declaration like `command: string`) is not a call
  }
  return { commands, unresolved };
}

/** The command of each `invoke(` call whose command can be read from the source. */
export function invokedCommands(text: string): string[] {
  return resolveInvokes(text).commands;
}

/**
 * Commands the UI reads while it renders. They must be faked: a rejection is often swallowed by the caller (a failed
 * observe_tab only shows "unavailable"), so a not-faked read goes unseen. src/fake/render.test.tsx finds them by opening
 * every page; this list is what that test found.
 */
export const RENDER_TIME_READS = [
  "observe_tab",
  "flight_export_preview",
  "flight_compare",
  "flight_eval_results",
  "flight_run_share",
  "dispatch_eval_preview",
];

const isTest = (path: string) => /\.test\.tsx?$/.test(path) || path.startsWith("/src/fake/");
const fixtureKeys = new Set(
  [app, clickup, run, runs, settings].flatMap((fixtures) => Object.keys(fixtures as Record<string, unknown>)),
);
const unfaked = notFaked as Record<string, string>;
const behaviourOnly = new Set<string>(BEHAVIOUR_ONLY);

describe("fake backend command coverage", () => {
  it("finds a command behind nested generics", () => {
    const text = `invoke<NonNullable<AutoPostView["record"]>>("clickup_auto_post_retry", { a }); invoke("plain")`;
    expect(invokedCommands(text)).toEqual(["clickup_auto_post_retry", "plain"]);
  });

  it("finds a command held in a constant or a template literal without substitutions", () => {
    const text = [
      'const READ = "read_thing";',
      "export const WRITE: string = 'write_thing';",
      "invoke(READ, { a }); invoke<Foo>(WRITE); invoke(`plain_tpl`); invoke<Map<string, Array<number>>>(\"nested\");",
      "function invoke(command: string, args: object) {}",
      "// invoke the command (see above)",
    ].join("\n");
    expect(invokedCommands(text)).toEqual(["read_thing", "write_thing", "plain_tpl", "nested"]);
    expect(resolveInvokes(text).unresolved).toEqual([]);
  });

  it("reports a command it cannot resolve", () => {
    const text = 'invoke(`dyn_${name}`); invoke(someVariable); invoke("ok_cmd");';
    expect(resolveInvokes(text)).toEqual({ commands: ["ok_cmd"], unresolved: ["`dyn_${name}`", "someVariable"] });
  });

  it("every invoke( in src/ has a command the scanner can read", () => {
    const bad = Object.entries(sources)
      .filter(([path]) => !isTest(path))
      .flatMap(([path, text]) => resolveInvokes(text).unresolved.map((arg) => `${path}: invoke(${arg})`));
    expect(bad, `the command of these calls cannot be read from the source: ${bad.join("; ")}`).toEqual([]);
  });

  it("render-time reads are faked, not listed as not-faked", () => {
    const stillUnfaked = RENDER_TIME_READS.filter((command) => command in unfaked);
    expect(stillUnfaked, `faked backend must answer: ${stillUnfaked.join(", ")}`).toEqual([]);
    for (const command of RENDER_TIME_READS) expect(fixtureKeys.has(command), `${command} has a fixture`).toBe(true);
  });

  it("every command invoked in src/ has a fixture or a not-faked reason", () => {
    const files = Object.entries(sources).filter(([path]) => !isTest(path));
    expect(files.length).toBeGreaterThan(50);
    const used = new Set(files.flatMap(([, text]) => invokedCommands(text)));
    expect(used.size).toBeGreaterThan(100);
    const missing = [...used].filter((c) => !fixtureKeys.has(c) && !behaviourOnly.has(c) && !(c in unfaked)).sort();
    expect(missing, `no fixture and no not-faked entry for: ${missing.join(", ")}`).toEqual([]);
  });

  it("test files are not scanned", () => {
    const files = Object.keys(sources).filter((path) => !isTest(path));
    expect(files.some((path) => path.endsWith(".test.ts") || path.endsWith(".test.tsx"))).toBe(false);
  });

  it("every not-faked entry has a reason and no fixture", () => {
    for (const [command, reason] of Object.entries(unfaked)) {
      expect(reason.trim().length, `${command} has an empty reason`).toBeGreaterThan(0);
      expect(fixtureKeys.has(command), `${command} is both faked and not-faked`).toBe(false);
      expect(behaviourOnly.has(command), `${command} is both faked by behaviour and not-faked`).toBe(false);
    }
  });

  it("a behaviour-only command has no fixture and is invoked by the UI", () => {
    const used = new Set(
      Object.entries(sources)
        .filter(([path]) => !isTest(path))
        .flatMap(([, text]) => invokedCommands(text)),
    );
    for (const command of BEHAVIOUR_ONLY) {
      expect(fixtureKeys.has(command), `${command} has a fixture: drop it from BEHAVIOUR_ONLY`).toBe(false);
      expect(used.has(command), `${command} is not invoked in src/`).toBe(true);
    }
  });
});
