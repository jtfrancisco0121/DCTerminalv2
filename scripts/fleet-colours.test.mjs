import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const SRC = join(dirname(fileURLToPath(import.meta.url)), "..", "src");
const css = readFileSync(join(SRC, "App.css"), "utf8");
const themes = [...readFileSync(join(SRC, "theme.ts"), "utf8").matchAll(/id: "([a-z-]+)"/g)].map((m) => m[1]).filter((id) => id !== "system");

function block(theme) {
  const head = theme === "github-dark" ? ":root {" : `:root[data-theme="${theme}"] {`;
  const start = css.indexOf(head);
  expect(start, `${theme} block`).toBeGreaterThanOrEqual(0);
  return css.slice(start, css.indexOf("\n}", start));
}

const fleetRules = [...css.replace(/\/\*[\s\S]*?\*\//g, "").matchAll(/(?:^|\n)(\.fleet-[^{]*)\{([^}]*)\}/g)].map((m) => ({ selector: m[1].trim(), body: m[2] }));

/** WCAG relative luminance of a #rrggbb colour. */
function luminance(hex) {
  const channel = (i) => {
    const c = parseInt(hex.slice(1 + i * 2, 3 + i * 2), 16) / 255;
    return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * channel(0) + 0.7152 * channel(1) + 0.0722 * channel(2);
}

const contrast = (a, b) => {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (hi + 0.05) / (lo + 0.05);
};

const token = (text, name) => new RegExp(`${name}: (#[0-9a-fA-F]{6});`).exec(text)?.[1];

describe("fleet timeline colours", () => {
  it("finds the fleet rules", () => {
    expect(fleetRules.length).toBeGreaterThan(5);
  });

  it("uses theme tokens only", () => {
    for (const rule of fleetRules) {
      expect(rule.body, rule.selector).not.toMatch(/#[0-9a-fA-F]{3,8}\b/);
      expect(rule.body, rule.selector).not.toContain("--panel-bg");
    }
    expect(css).not.toContain("--panel-bg");
  });

  it("draws the wait hatch as --warning stripes over --warning-subtle", () => {
    const wait = fleetRules.find((rule) => rule.selector === ".fleet-wait");
    expect(wait?.body).toContain("var(--warning) ");
    expect(wait?.body).toContain("var(--warning-subtle)");
  });

  it.each(themes)("%s keeps --warning at 3:1 against the --bg-muted track", (theme) => {
    const text = block(theme);
    const warning = token(text, "--warning");
    const track = token(text, "--bg-muted");
    expect(warning).toBeTruthy();
    expect(track).toBeTruthy();
    expect(contrast(warning, track)).toBeGreaterThanOrEqual(3);
  });
});
