import { describe, expect, it } from "vitest";
import app from "../../fixtures/fake/app.json";
import clickup from "../../fixtures/fake/clickup.json";
import notFaked from "../../fixtures/fake/not-faked.json";
import run from "../../fixtures/fake/run.json";
import runs from "../../fixtures/fake/runs.json";
import settings from "../../fixtures/fake/settings.json";

// Every command the UI invokes is either faked (a fixture) or listed with a reason in fixtures/fake/not-faked.json.
// Adding an `invoke("new_command")` in src/ without one of the two fails here, naming the command.
const sources = import.meta.glob("/src/**/*.{ts,tsx}", { query: "?raw", import: "default", eager: true }) as Record<
  string,
  string
>;

/** The command of each `invoke(` call: the first string literal after the call's `(` (type arguments may nest). */
export function invokedCommands(text: string): string[] {
  const found: string[] = [];
  for (const m of text.matchAll(/\binvoke\b/g)) {
    const rest = text.slice(m.index! + m[0].length);
    const open = rest.indexOf("(");
    if (open < 0 || open > 400) continue;
    const literal = /^\s*["'`]([A-Za-z0-9_:|]+)["'`]/.exec(rest.slice(open + 1));
    if (literal) found.push(literal[1]!);
  }
  return found;
}

const isTest = (path: string) => /\.test\.tsx?$/.test(path) || path.startsWith("/src/fake/");
const fixtureKeys = new Set(
  [app, clickup, run, runs, settings].flatMap((fixtures) => Object.keys(fixtures as Record<string, unknown>)),
);
const unfaked = notFaked as Record<string, string>;

describe("fake backend command coverage", () => {
  it("finds a command behind nested generics", () => {
    const text = `invoke<NonNullable<AutoPostView["record"]>>("clickup_auto_post_retry", { a }); invoke("plain")`;
    expect(invokedCommands(text)).toEqual(["clickup_auto_post_retry", "plain"]);
  });

  it("every command invoked in src/ has a fixture or a not-faked reason", () => {
    const files = Object.entries(sources).filter(([path]) => !isTest(path));
    expect(files.length).toBeGreaterThan(50);
    const used = new Set(files.flatMap(([, text]) => invokedCommands(text)));
    expect(used.size).toBeGreaterThan(100);
    const missing = [...used].filter((c) => !fixtureKeys.has(c) && !(c in unfaked)).sort();
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
    }
  });
});
