// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { readSource } from "./readSource";
import { FOLDS_KEY, loadFolds, MAX_FOLDED_RUNS, saveFolds } from "./runPanelFolds";

beforeEach(() => localStorage.clear());
afterEach(() => vi.restoreAllMocks());

const stored = () => JSON.parse(localStorage.getItem(FOLDS_KEY) ?? "{}") as Record<string, { at: number; folded: string[] }>;

describe("saved folds per run", () => {
  it("remembers a run's folded sections and gives none for an unknown run", () => {
    expect(loadFolds("r-1")).toEqual([]);
    saveFolds("r-1", ["timeline", "agents"]);
    saveFolds("r-2", ["cleanup"]);
    expect(loadFolds("r-1")).toEqual(["timeline", "agents"]);
    expect(loadFolds("r-2")).toEqual(["cleanup"]);
    saveFolds("r-1", []);
    expect(loadFolds("r-1")).toEqual([]);
    expect(Object.keys(stored())).toEqual(["r-2"]);
  });

  it("keeps only the newest 50 runs on every write", () => {
    for (let i = 0; i < MAX_FOLDED_RUNS + 5; i++) saveFolds(`r-${i}`, ["timeline"], 1000 + i);
    const runs = Object.keys(stored());
    expect(runs).toHaveLength(MAX_FOLDED_RUNS);
    expect(runs).not.toContain("r-0");
    expect(runs).not.toContain("r-4");
    expect(runs).toContain("r-5");
    expect(runs).toContain(`r-${MAX_FOLDED_RUNS + 4}`);
    // An older run touched again becomes the newest and survives the next prune.
    saveFolds("r-5", ["timeline", "prs"], 9999);
    saveFolds("r-new", ["plan"], 10000);
    expect(Object.keys(stored())).toContain("r-5");
    expect(Object.keys(stored())).not.toContain("r-6");
  });

  it("treats junk JSON, junk entries and unknown section ids as no folds", () => {
    localStorage.setItem(FOLDS_KEY, "{not json");
    expect(loadFolds("r-1")).toEqual([]);
    localStorage.setItem(FOLDS_KEY, JSON.stringify([1, 2]));
    expect(loadFolds("r-1")).toEqual([]);
    localStorage.setItem(FOLDS_KEY, JSON.stringify({ "r-1": "x", "r-2": { at: "no", folded: ["timeline"] }, "r-3": { at: 1, folded: ["timeline", "bogus", 7] } }));
    expect(loadFolds("r-1")).toEqual([]);
    expect(loadFolds("r-2")).toEqual([]);
    expect(loadFolds("r-3")).toEqual(["timeline"]);
    saveFolds("r-4", ["prs"]);
    expect(loadFolds("r-4")).toEqual(["prs"]);
  });

  it("never throws when storage does", () => {
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
      throw new Error("denied");
    });
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new Error("denied");
    });
    expect(loadFolds("r-1")).toEqual([]);
    expect(() => saveFolds("r-1", ["timeline"])).not.toThrow();
  });

  it("never writes the per-window layout key", () => {
    saveFolds("r-1", ["timeline"]);
    const keys = Array.from({ length: localStorage.length }, (_, i) => localStorage.key(i));
    expect(keys).toEqual([FOLDS_KEY]);
    expect(keys.some((key) => key?.startsWith("dcv2.runPanel."))).toBe(false);
  });
});

describe("fold motion CSS", () => {
  const css = readSource("App.css").replace(/\/\*[\s\S]*?\*\//g, "");
  const body = (selector: string) => new RegExp(`\\n${selector.replace(/[.[\]]/g, "\\$&")} \\{([^}]*)\\}`).exec(css)?.[1] ?? "";

  it("animates grid-template-rows over --dur-fold (200ms) with the existing easing", () => {
    expect(body(".fold")).toMatch(/--dur-fold: 200ms;/);
    expect(body(".fold-body")).toMatch(/grid-template-rows: 0fr;/);
    expect(body(".fold-body")).toMatch(/transition: grid-template-rows var\(--dur-fold\) var\(--ease\);/);
    expect(body(".fold-open > .fold-body")).toMatch(/grid-template-rows: 1fr;/);
  });

  it("defines --dur-fold inside the marked block only, not in :root", () => {
    const text = readSource("App.css");
    const start = text.indexOf("/* ---- Run panel folds");
    const end = text.indexOf("/* ---- end Run panel folds ---- */");
    expect(start).toBeGreaterThan(0);
    expect(end).toBeGreaterThan(start);
    const defs = [...text.matchAll(/--dur-fold:/g)].map((m) => m.index!);
    expect(defs.length).toBeGreaterThan(0);
    for (const at of defs) expect(at > start && at < end).toBe(true);
  });

  it("is covered by the one reduced-motion rule, which is not edited to name it", () => {
    const at = css.indexOf("@media (prefers-reduced-motion: reduce)");
    const rule = css.slice(at, css.indexOf("\n}\n", at));
    expect(rule).toMatch(/\*,\s*\*::before,\s*\*::after \{\s*transition: none !important;/);
    expect(rule).not.toMatch(/fold/);
  });
});
