// The preview's width is the flex basis PreviewHost fits from `previewFit` (it keeps 60 terminal columns). jsdom cannot
// see a CSS rule beating an inline style, so this reads `src/App.css` as text and fails when any `.preview-pane` rule
// sets a basis, a width or a minimum of its own, or when the old Wider rules or the 1000px media rule come back.
import { describe, expect, it } from "vitest";

type Fs = { readFileSync: (path: string, encoding: "utf8") => string };
const fsName = "node:fs";
const fs = (await import(/* @vite-ignore */ fsName)) as Fs;
const css = fs.readFileSync(decodeURIComponent(new URL("../App.css", import.meta.url).pathname), "utf8").replace(/\/\*[\s\S]*?\*\//g, "");

const rules = [...css.matchAll(/([^{}]+)\{([^{}]*)\}/g)].map((m) => ({
  selectors: m[1].split(",").map((s) => s.trim()),
  body: m[2],
}));
const previewRules = rules.filter((r) => r.selectors.some((s) => /(^|[\s>])\.preview-pane(?![\w-])/.test(s)));
const decl = (body: string, name: string) => [...body.matchAll(new RegExp(`(?:^|[\\s;])${name}\\s*:\\s*([^;]+)`, "g"))].map((m) => m[1].trim());

describe(".preview-pane rules", () => {
  it("finds the rules", () => {
    expect(previewRules.length).toBeGreaterThan(0);
    expect(previewRules.some((r) => r.selectors.includes(".preview-pane"))).toBe(true);
  });
  it("sets no flex-basis, no width and no minimum other than 0", () => {
    for (const rule of previewRules) {
      const where = rule.selectors.join(", ");
      expect(decl(rule.body, "flex-basis"), `${where} flex-basis`).toEqual([]);
      expect(decl(rule.body, "width"), `${where} width`).toEqual([]);
      for (const min of decl(rule.body, "min-width")) expect(min, `${where} min-width`).toMatch(/^0(px|rem)?$/);
      for (const flex of decl(rule.body, "flex")) {
        // `flex: <grow> <shrink> <basis>`: a non-zero basis (or `auto`) would beat the fitted width.
        const basis = flex.split(/\s+/)[2];
        expect(basis === undefined || /^0(px|%)?$/.test(basis), `${where} flex: ${flex}`).toBe(true);
        expect(flex, `${where} flex`).not.toMatch(/^(auto|none|\d+(\.\d+)?)$/);
      }
    }
  });
  it("has no Wider rule and no 1000px media rule for the preview", () => {
    expect(css).not.toContain(".preview-pane.wide");
    expect(css).not.toMatch(/@media[^{]*max-width:\s*1000px[^{]*\{\s*\.preview-pane/);
  });
});
