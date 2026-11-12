// The tab row's overflow layout, read from `src/App.css` as text (jsdom lays nothing out). QA S5.1: at 420px the chosen
// tab was wider than the row and could never be shown whole, so "+1" kept counting it; at 320px the strip shrank under
// the "+N" button and the drag spacer after it covered the button.
import { describe, expect, it } from "vitest";

type Fs = { readFileSync: (path: string, encoding: "utf8") => string };
const fsName = "node:fs";
const fs = (await import(/* @vite-ignore */ fsName)) as Fs;
const css = fs.readFileSync(decodeURIComponent(new URL("../App.css", import.meta.url).pathname), "utf8");
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

describe("tab row overflow layout", () => {
  it("a tab is never wider than the row's view, so scrolling it into view shows it whole", () => {
    // TabBar measures the row and sets --tab-view on it; a percentage would be ignored (the row's width comes from its tabs).
    expect(body(".tab")).toMatch(/max-width:\s*min\([^;]*var\(--tab-view[^;]*\)\s*;/);
  });

  it("a narrow window hides the account name and shrinks the search, leaving the tabs room", () => {
    const narrow = /@media \(max-width: 40rem\) \{([\s\S]*?)\n\}/.exec(css)?.[1] ?? "";
    expect(narrow).toMatch(/\.title-bar-account \{\s*display: none;/);
    expect(narrow).toMatch(/\.title-bar-search \{\s*min-width: 0;/);
  });

  it("the tab row shows no scrollbar", () => {
    expect(body(".tab-bar")).toMatch(/scrollbar-width:\s*none\s*;/);
  });

  it("the strip never shrinks under the +N button: the list column shrinks to 0, the button's column does not", () => {
    const strip = body(".tab-strip");
    expect(strip).toMatch(/display:\s*grid\s*;/);
    expect(strip).toMatch(/grid-template-columns:\s*minmax\(0,\s*auto\)\s+auto\s*;/);
    // A min-width of 0 would let the strip shrink under its min-content (the button) again.
    expect(strip + body(".title-bar .tab-strip")).not.toMatch(/min-width:\s*0\s*;/);
  });
});
