// The start screen's role tiles at the narrowest supported window (800 px): jsdom cannot lay anything out, so this reads
// `src/App.css` as text and computes the grid's columns from the rules. It fails when it cannot find or parse a rule
// ("cannot compute" is a failure, never a pass), when a tile may clip or truncate its label, or when fewer than 3
// columns fit. It assumes the start screen spans the window width (`.start-screen` is a direct child of the app
// layout), so a narrower container needs this test updated.
import { describe, expect, it } from "vitest";

// Read with node's fs (the repo has no @types/node, so the module names go through variables) because vitest
// returns an empty string for a CSS file read through `?raw`.
type Fs = { readFileSync: (path: string, encoding: "utf8") => string };
const fsName = "node:fs";
const fs = (await import(/* @vite-ignore */ fsName)) as Fs;
const css = fs.readFileSync(
  decodeURIComponent(new URL("../App.css", import.meta.url).pathname),
  "utf8",
);

const WINDOW_PX = 800;
const MIN_COLUMNS = 3;
const REM_PX = 16;

type Rule = { selectors: string[]; body: string };

const rules: Rule[] = [
  ...css.replace(/\/\*[\s\S]*?\*\//g, "").matchAll(/([^{}]+)\{([^{}]*)\}/g),
].map((m) => ({
  selectors: m[1].split(",").map((s) => s.trim()),
  body: m[2],
}));

/** The rules with a selector for `cls` itself (`.cls`, `.cls:hover`, `.parent .cls`), never `.cls-more`. */
const rulesFor = (cls: string) =>
  rules.filter((r) =>
    r.selectors.some((s) => new RegExp(`\\.${cls}(?![\\w-])`).test(s)),
  );

const rule = (cls: string) => {
  const found = rulesFor(cls).filter((r) => r.selectors.includes(`.${cls}`));
  expect(found.length, `no .${cls} rule in App.css`).toBeGreaterThan(0);
  return found[0].body;
};

const rem = (text: string, what: string) => {
  const n = Number(text);
  expect(Number.isFinite(n), `cannot read ${what}`).toBe(true);
  return n * REM_PX;
};

/** The number of columns `repeat(auto-fill, minmax(min, 1fr))` gives at `WINDOW_PX`, from the three rules. */
const columnsAtWindow = () => {
  const padding = rule("start-screen").match(
    /padding:\s*[^;]*max\(\s*[\d.]+rem\s*,\s*calc\(\s*50%\s*-\s*([\d.]+)rem\s*\)\s*\)/,
  );
  expect(
    padding,
    "cannot read the .start-screen side padding cap",
  ).not.toBeNull();
  const side = Math.max(0, WINDOW_PX / 2 - rem(padding![1], "the padding cap"));
  const content = WINDOW_PX - 2 * Math.max(0, side);

  const grid = rule("role-tiles");
  const minmax = grid.match(
    /grid-template-columns:\s*repeat\(\s*auto-fill\s*,\s*minmax\(\s*([\d.]+)rem\s*,/,
  );
  expect(minmax, "cannot read the .role-tiles minmax minimum").not.toBeNull();
  const min = rem(minmax![1], "the minmax minimum");

  const gapText = grid.match(/(?:^|[\s;])gap:\s*([^;]+);?/)?.[1].trim();
  expect(gapText, "cannot read the .role-tiles gap").toBeTruthy();
  const variable = gapText!.match(/^var\(\s*(--[\w-]+)\s*\)$/);
  let gapRem = gapText!.match(/^([\d.]+)rem$/)?.[1];
  if (variable) {
    const root = rules.find((r) => r.selectors.includes(":root"));
    gapRem = root?.body.match(
      new RegExp(`${variable[1]}:\\s*([\\d.]+)rem`),
    )?.[1];
  }
  expect(gapRem, `cannot resolve the gap ${gapText}`).toBeTruthy();
  const gap = rem(gapRem!, "the gap");

  return Math.floor((content + gap) / (min + gap));
};

describe("start screen role tiles at 800 px", () => {
  it("finds the three rules", () => {
    rule("start-screen");
    rule("role-tiles");
    rule("role-tile");
  });

  it("never clips or truncates a tile's label", () => {
    for (const r of rulesFor("role-tile")) {
      expect(
        r.body,
        `${r.selectors.join(", ")} must let the label wrap`,
      ).not.toMatch(/(^|[\s;])(white-space|overflow|text-overflow)\s*:/);
    }
  });

  it(`fits at least ${MIN_COLUMNS} columns`, () => {
    expect(columnsAtWindow()).toBeGreaterThanOrEqual(MIN_COLUMNS);
  });
});
