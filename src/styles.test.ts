import { describe, expect, it } from "vitest";
import { readSource } from "./readSource";

/** App.css as text: rules about tokens and layout that no render test can see. */
const css = readSource("App.css");
const noComments = css.replace(/\/\*[\s\S]*?\*\//g, "");

/** Every `--name: value;` definition in the file. */
function definitions(name: string): string[] {
  return [...noComments.matchAll(new RegExp(`^\\s*${name}: ([^;]+);`, "gm"))].map((m) => m[1].trim());
}

describe("rhythm tokens (S2)", () => {
  it("sets the corner radii to 8, 12 and 14px and the gutter to 8px", () => {
    expect(definitions("--radius-sm")).toEqual(["8px"]);
    expect(definitions("--radius-md")).toEqual(["12px"]);
    expect(definitions("--radius-lg")).toEqual(["14px"]);
    expect(definitions("--gutter")).toEqual(["8px"]);
  });

  it("explains each fixed px token on its own line", () => {
    for (const name of ["--radius-sm", "--radius-md", "--radius-lg", "--gutter"]) {
      expect(css).toMatch(new RegExp(`${name}: \\d+px; /\\* px: [^*]+\\*/`));
    }
  });

  it("defines each --space-* once, in rem, on the 4px grid at 100%", () => {
    const names = [...new Set([...noComments.matchAll(/^\s*(--space-\d+):/gm)].map((m) => m[1]))];
    expect(names.length).toBeGreaterThanOrEqual(6);
    for (const name of names) {
      const values = definitions(name);
      expect(values, name).toHaveLength(1);
      const rem = /^(\d*\.?\d+)rem$/.exec(values[0]);
      expect(rem, `${name} is in rem`).not.toBeNull();
      expect((Number(rem![1]) * 16) % 4, name).toBe(0);
    }
  });

  it("has at most 51 `1px solid` declarations", () => {
    expect((noComments.match(/1px solid/g) ?? []).length).toBeLessThanOrEqual(51);
  });

  it("separates the run layout's panes with the gutter, not a line", () => {
    expect(noComments).toMatch(/\.run-layout \{[^}]*gap: var\(--gutter\)/);
    for (const selector of [".run-panel", ".run-rail", ".preview-pane"]) {
      const body = new RegExp(`\\n\\${selector} \\{([^}]*)\\}`).exec(noComments)?.[1] ?? "";
      expect(body, selector).not.toMatch(/border-left/);
      expect(body, selector).toMatch(/background: var\(--bg-elev\)/);
    }
  });

  it("sets tabular numbers on .num and the cost and count classes", () => {
    const rule = /((?:[.\w-]+,\s*)+[.\w-]+) \{\s*font-variant-numeric: tabular-nums;/.exec(noComments);
    expect(rule).not.toBeNull();
    const selectors = rule![1].split(",").map((s) => s.trim());
    for (const wanted of [".num", ".run-cost", ".cost-seg", ".error-count", ".replay-track-count"]) {
      expect(selectors).toContain(wanted);
    }
  });
});

describe("size tokens follow the UI scale (S4)", () => {
  it("defines every --space-* and --text-* token in rem (em for --text-rel-*)", () => {
    const tokens = [...noComments.matchAll(/^\s*(--(?:space|text)-[a-z0-9-]+): ([^;]+);/gm)];
    expect(tokens.length).toBeGreaterThanOrEqual(14);
    for (const [, name, value] of tokens) {
      const unit = name.startsWith("--text-rel-") ? "em" : "rem";
      expect(value.trim(), name).toMatch(new RegExp(`^\\d*\\.?\\d+${unit}$`));
    }
  });

  it("has no raw font-size outside :root", () => {
    const raw = [...noComments.matchAll(/font-size:\s*([^;]+);/g)].filter((m) => !/^(var\(.*\)|inherit)$/.test(m[1].trim()));
    expect(raw.map((m) => m[0])).toEqual([]);
  });
});
