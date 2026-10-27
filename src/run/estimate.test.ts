import { describe, expect, it } from "vitest";
import type { RunListItem } from "./api";
import { estimate, estimateText } from "./estimate";

let n = 0;
const run = (over: Partial<RunListItem> & { usd?: number | null } = {}): RunListItem => {
  const { usd = 5, ...rest } = over;
  n += 1;
  return {
    runId: `r-${n}`,
    runDir: `/d/r-${n}`,
    folder: "/w",
    createdAt: null,
    stage: "done",
    updatedAt: null,
    title: null,
    repoCount: 1,
    bmadApplied: false,
    record: usd === null ? null : { outcome: "done", totalTokens: 1, estimateUsd: usd, minutes: 1, prUrls: [], firstActivity: null, lastActivity: null },
    ...rest,
  };
};

describe("estimate", () => {
  it("needs three matching finished runs and gives their range", () => {
    expect(estimate([run({ usd: 4 }), run({ usd: 9 })], 1, false)).toBeNull();
    expect(estimate([run({ usd: 4 }), run({ usd: 9 }), run({ usd: 6 })], 1, false)).toEqual({ low: 4, high: 9, n: 3 });
  });

  it("matches the repo bucket: 1, 2-3, 4+", () => {
    const runs = [run({ repoCount: 1 }), run({ repoCount: 2 }), run({ repoCount: 3 }), run({ repoCount: 4 }), run({ repoCount: 7 })];
    expect(estimate(runs, 2, false)).toBeNull();
    expect(estimate([...runs, run({ repoCount: 3 })], 2, false)?.n).toBe(3);
    expect(estimate([...runs, run({ repoCount: 5 })], 6, false)?.n).toBe(3);
  });

  it("matches the BMAD flag, and both kinds when the mode is auto", () => {
    const runs = [run({ bmadApplied: true }), run({ bmadApplied: true }), run({ bmadApplied: false }), run({ bmadApplied: null })];
    expect(estimate(runs, 1, true)).toBeNull();
    expect(estimate(runs, 1, false)).toBeNull(); // false and unknown count as no BMAD: 2 runs
    expect(estimate([...runs, run({ bmadApplied: false })], 1, false)?.n).toBe(3);
    expect(estimate(runs, 1, null)?.n).toBe(4);
  });

  it("takes only finished runs with an estimate, newest 20", () => {
    const old = Array.from({ length: 5 }, () => run({ usd: 100 }));
    const fresh = Array.from({ length: 20 }, () => run({ usd: 2 }));
    expect(estimate([...fresh, ...old], 1, false)).toEqual({ low: 2, high: 2, n: 20 });
    const skipped = [run({ stage: "building" }), run({ usd: null }), run({ usd: 3 }), run({ stage: "failed", usd: 3 }), run({ usd: 3 })];
    expect(estimate(skipped, 1, false)?.n).toBe(3);
  });

  it("words the line", () => {
    expect(estimateText({ low: 4.2, high: 8.6, n: 6 })).toBe("Similar runs: $4–$9 (6 runs)");
    expect(estimateText(null)).toBe("Not enough earlier runs for an estimate.");
  });
});
