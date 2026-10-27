import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const SRC = join(dirname(fileURLToPath(import.meta.url)), "..", "src");
const css = readFileSync(join(SRC, "App.css"), "utf8");
const themes = [...readFileSync(join(SRC, "theme.ts"), "utf8").matchAll(/id: "([a-z-]+)"/g)].map((m) => m[1]).filter((id) => id !== "system");

/** The declarations of one theme's block (the default theme is the bare `:root`). */
function block(theme) {
  const head = theme === "github-dark" ? ":root {" : `:root[data-theme="${theme}"] {`;
  const start = css.indexOf(head);
  expect(start, `${theme} block`).toBeGreaterThanOrEqual(0);
  return css.slice(start, css.indexOf("\n}", start));
}

const value = (text, name) => new RegExp(`${name}: ([^;]+);`).exec(text)?.[1]?.trim();

describe("the waiting dot colour", () => {
  it("covers all six themes", () => {
    expect(themes).toHaveLength(6);
  });

  it.each(themes)("%s defines --activity-waiting apart from the working and needs-you colours", (theme) => {
    const text = block(theme);
    expect(value(text, "--activity-waiting")).toBe("var(--accent-fg)");
    const accent = value(text, "--accent-fg");
    expect(accent).toBeTruthy();
    expect(accent).not.toBe(value(text, "--success"));
    expect(accent).not.toBe(value(text, "--warning"));
  });

  it("styles the waiting dot with it", () => {
    expect(css).toMatch(/\.tab-activity-waiting \{\s*background: var\(--activity-waiting\);/);
  });
});
