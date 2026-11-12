// A run tab has two lines (name and stage word) plus the progress line. jsdom cannot lay anything out, so this reads
// `src/App.css` as text and computes the tab's height at every UI scale from its rules; the row it sits in is
// `--titlebar-h` tall, in px, and does not scale. It fails when the tab is taller than the row and when it cannot
// find or parse a rule ("cannot compute" is a failure, never a pass).
import { describe, expect, it } from "vitest";
import { UI_SCALES } from "../uiScale";

type Fs = { readFileSync: (path: string, encoding: "utf8") => string };
const fsName = "node:fs";
const fs = (await import(/* @vite-ignore */ fsName)) as Fs;
const css = fs.readFileSync(decodeURIComponent(new URL("../App.css", import.meta.url).pathname), "utf8");

const BASE_REM_PX = 16;
const stripped = css.replace(/\/\*[\s\S]*?\*\//g, "");
const rules = [...stripped.matchAll(/([^{}]+)\{([^{}]*)\}/g)].map((m) => ({
  selectors: m[1].split(",").map((s) => s.trim()),
  body: m[2],
}));

const body = (selector: string) => {
  const found = rules.filter((r) => r.selectors.includes(selector));
  expect(found.length, `no ${selector} rule in App.css`).toBeGreaterThan(0);
  return found.map((r) => r.body).join("\n");
};
const num = (text: string | undefined, what: string) => {
  const n = Number(text);
  expect(Number.isFinite(n) && text !== undefined, `cannot read ${what}`).toBe(true);
  return n;
};
const token = (name: string, unit: "rem" | "px") => {
  const match = stripped.match(new RegExp(`${name}:\\s*([\\d.]+)${unit}\\s*;`));
  return num(match?.[1], `the ${name} token`);
};
const prop = (selector: string, name: string, unit: string) =>
  num(body(selector).match(new RegExp(`(?:^|[\\s;])${name}:\\s*([\\d.]+)${unit}\\s*;`))?.[1], `${selector} ${name}`);

/** The run tab's height in px at `scale` percent: bar top padding, label padding, two lines, the progress line. */
function runTabHeight(scale: number): number {
  const rem = (BASE_REM_PX * scale) / 100;
  const barTop = num(body(".tab-bar").match(/padding:\s*([\d.]+)rem\s/)?.[1], ".tab-bar top padding");
  const pad = prop(".tab-label-run", "padding-top", "rem") + prop(".tab-label-run", "padding-bottom", "rem");
  const nameLine = token("--text-sm", "rem") * prop(".tab-name", "line-height", "");
  const stageLine = token("--text-2xs", "rem") * prop(".tab-stage", "line-height", "");
  expect(body(".tab-stage")).toMatch(/font-size:\s*var\(--text-2xs\)/);
  expect(body(".tab-label")).toMatch(/font-size:\s*var\(--text-sm\)/);
  return (barTop + pad + nameLine + stageLine) * rem + prop(".tab-progress", "height", "px");
}

describe("the run tab fits the title-bar row", () => {
  const row = token("--titlebar-h", "px");
  it("draws the progress line 3px tall", () => {
    expect(prop(".tab-progress", "height", "px")).toBe(3);
  });
  for (const scale of UI_SCALES) {
    it(`at ${scale}%`, () => {
      expect(runTabHeight(scale)).toBeLessThanOrEqual(row);
    });
  }
});
