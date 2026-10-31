import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const SRC = join(dirname(fileURLToPath(import.meta.url)), "..", "src");
const css = readFileSync(join(SRC, "App.css"), "utf8").replace(/\/\*[\s\S]*?\*\//g, "");

/** Every `selector { declarations }` pair, flat (at-rule wrappers are walked into). */
const rules = [...css.matchAll(/([^{}]+)\{([^{}]*)\}/g)].map((m) => ({ selector: m[1].trim(), body: m[2] }));
const declared = (property) =>
  rules.flatMap((rule) => [...rule.body.matchAll(new RegExp(`(?:^|[;\\s])${property}:\\s*([^;]+);`, "g"))].map((m) => ({ selector: rule.selector, value: m[1].trim() })));

describe("motion", () => {
  it("defines the tokens on :root", () => {
    const root = rules.find((rule) => rule.selector === ":root");
    expect(root?.body).toMatch(/--ease: cubic-bezier\(/);
    expect(root?.body).toMatch(/--dur-fast: 120ms;/);
    expect(root?.body).toMatch(/--dur-spin: 900ms;/);
  });

  it("disables transitions and animations for reduced motion", () => {
    const at = css.indexOf("@media (prefers-reduced-motion: reduce)");
    expect(at).toBeGreaterThanOrEqual(0);
    const inner = css.slice(at, css.indexOf("\n}\n", at));
    expect(inner).toMatch(/transition: none !important;/);
    expect(inner).toMatch(/animation: none !important;/);
  });

  it("never moves the terminal", () => {
    for (const rule of rules) {
      if (/terminal-view|terminal-slot|xterm/.test(rule.selector)) {
        expect(rule.body, rule.selector).not.toMatch(/(?:^|[;\s])(?:transition|animation)\b/);
      }
    }
  });

  it("transitions never list width or all, and use the fast duration", () => {
    const transitions = declared("transition");
    expect(transitions.length).toBeGreaterThan(0);
    for (const { selector, value } of transitions) {
      if (value.startsWith("none")) continue;
      expect(value, selector).not.toMatch(/\b(?:width|all)\b/);
      expect(value, selector).toContain("var(--dur-fast)");
      expect(value.replace(/var\(--dur-fast\)|var\(--ease\)/g, ""), selector).not.toMatch(/\d+m?s/);
    }
  });

  it("animations use the fast duration, the spinner its own", () => {
    const animations = declared("animation").filter(({ value }) => !value.startsWith("none"));
    expect(animations.length).toBeGreaterThan(0);
    for (const { selector, value } of animations) {
      const want = selector === ".icon-spin" ? "var(--dur-spin)" : "var(--dur-fast)";
      expect(value, selector).toContain(want);
      expect(value.replace(/var\(--[a-z-]+\)/g, ""), selector).not.toMatch(/\d+m?s/);
    }
  });
});
