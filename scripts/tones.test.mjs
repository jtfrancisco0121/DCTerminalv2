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

const TONES = ["success", "warn", "danger", "running", "muted", "you"];

/** The declaration text of every rule whose selector list is exactly `.tone-<name>`. */
function toneRules(name) {
  return [...css.matchAll(new RegExp(`(?:^|\\n)\\.tone-${name} \\{([^}]*)\\}`, "g"))].map((m) => m[1]);
}

describe("tone classes", () => {
  it("has one rule per tone", () => {
    for (const tone of TONES) expect(toneRules(tone), tone).toHaveLength(1);
  });

  it.each(themes)("%s defines every token the tones use", (theme) => {
    const text = block(theme) + (theme === "github-dark" ? "" : block("github-dark"));
    for (const tone of TONES) {
      const tokens = [...toneRules(tone)[0].matchAll(/var\((--[a-z-]+)\)/g)].map((m) => m[1]);
      expect(tokens.length, tone).toBeGreaterThan(0);
      for (const token of tokens) expect(text, `${theme} ${token}`).toMatch(new RegExp(`${token}: [^;]+;`));
    }
  });
});
