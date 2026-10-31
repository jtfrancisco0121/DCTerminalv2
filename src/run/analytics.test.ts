import { describe, expect, it } from "vitest";
import type { RunListItem } from "./api";
import { filterOptions, median, runsSummary, sortRuns, trendSeries } from "./analytics";

const NOW = new Date("2026-11-01T12:00:00Z");

function run(id: string, createdAt: string | null, record: Partial<NonNullable<RunListItem["record"]>> | null, extra: Partial<RunListItem> = {}): RunListItem {
  return {
    runId: id,
    runDir: `/d/${id}`,
    folder: "/w/app",
    createdAt,
    stage: "done",
    updatedAt: null,
    title: id,
    record: record && {
      outcome: "done",
      totalTokens: 0,
      estimateUsd: 1,
      minutes: 10,
      prUrls: [],
      firstActivity: null,
      lastActivity: null,
      ...record,
    },
    ...extra,
  };
}

describe("median", () => {
  it("handles empty, odd and even counts", () => {
    expect(median([])).toBeNull();
    expect(median([3, 1, 2])).toBe(2);
    expect(median([4, 1, 3, 2])).toBe(2.5);
  });
});

describe("runsSummary", () => {
  it("gives no cost figure and no median for an empty set", () => {
    expect(runsSummary([], NOW)).toMatchObject({ count: 0, estimateUsd: null, unpriced: 0, medianMinutes: null });
  });

  it("counts the window, sums priced estimates, counts unpriced and takes the median", () => {
    const runs = [
      run("a", "2026-10-31T10:00:00Z", { estimateUsd: 2, minutes: 10 }),
      run("b", "2026-10-30T10:00:00Z", { estimateUsd: 3.5, minutes: 30 }),
      run("c", "2026-10-29T10:00:00Z", { estimateUsd: null, minutes: 20 }),
      run("old", "2026-10-01T10:00:00Z", { estimateUsd: 99, minutes: 999 }),
      run("open", "2026-10-31T11:00:00Z", null, { stage: "building" }),
    ];
    const s = runsSummary(runs, NOW);
    expect(s).toMatchObject({ count: 4, estimateUsd: 5.5, unpriced: 1, medianMinutes: 20 });
  });

  it("shows a dash value (null), never 0, when nothing is priced", () => {
    const s = runsSummary([run("a", "2026-10-31T10:00:00Z", { estimateUsd: null })], NOW);
    expect(s.estimateUsd).toBeNull();
    expect(s.unpriced).toBe(1);
  });
});

describe("sortRuns", () => {
  const runs = [
    run("a", "2026-10-29T10:00:00Z", { estimateUsd: 2, minutes: 30 }),
    run("b", "2026-10-30T10:00:00Z", { estimateUsd: null, minutes: 10 }),
    run("c", "2026-10-31T10:00:00Z", { estimateUsd: 5, minutes: 20 }),
    run("d", "2026-10-28T10:00:00Z", null),
  ];
  const ids = (list: RunListItem[]) => list.map((r) => r.runId);

  it("is newest first by default", () => {
    expect(ids(sortRuns(runs, null))).toEqual(["c", "b", "a", "d"]);
  });

  it("puts runs lacking the value last in both directions", () => {
    expect(ids(sortRuns(runs, "estimate", "asc"))).toEqual(["a", "c", "b", "d"]);
    expect(ids(sortRuns(runs, "estimate", "desc"))).toEqual(["c", "a", "b", "d"]);
    expect(ids(sortRuns(runs, "duration", "asc"))).toEqual(["b", "c", "a", "d"]);
    expect(ids(sortRuns(runs, "duration", "desc"))).toEqual(["a", "c", "b", "d"]);
  });

  it("does not change its input", () => {
    const before = ids(runs);
    sortRuns(runs, "duration", "asc");
    expect(ids(runs)).toEqual(before);
  });
});

describe("trendSeries and filterOptions", () => {
  const runs = [
    run("a", "2026-10-29T10:00:00Z", { estimateUsd: 2, minutes: 30, waitMinutes: 10, spanMinutes: 40 }, { repos: ["api"] }),
    run("b", "2026-10-28T10:00:00Z", { estimateUsd: null, minutes: 10, waitMinutes: 0, spanMinutes: 0 }, { repos: ["web"], folder: "/w/other" }),
    run("c", "2026-10-30T10:00:00Z", { estimateUsd: 4, minutes: 20 }, { repos: ["api", "web"] }),
    run("open", "2026-10-31T10:00:00Z", null, { stage: "building" }),
  ];

  it("orders finished runs oldest first and counts what it skipped", () => {
    const t = trendSeries(runs);
    expect(t.estimate.map((p) => p.runId)).toEqual(["a", "c"]);
    expect(t.minutes.map((p) => p.runId)).toEqual(["b", "a", "c"]);
    expect(t.waitShare.map((p) => [p.runId, p.value])).toEqual([["a", 0.25]]);
    expect(t.skipped).toEqual({ estimate: 1, minutes: 0, waitShare: 2 });
  });

  it("narrows by folder and by repo", () => {
    expect(trendSeries(runs, { folder: "/w/other" }).minutes.map((p) => p.runId)).toEqual(["b"]);
    expect(trendSeries(runs, { repo: "web" }).minutes.map((p) => p.runId)).toEqual(["b", "c"]);
    expect(trendSeries(runs, { repo: "nope" }).minutes).toEqual([]);
  });

  it("lists distinct folders and repos", () => {
    expect(filterOptions(runs)).toEqual({ folders: ["/w/app", "/w/other"], repos: ["api", "web"] });
    expect(filterOptions([])).toEqual({ folders: [], repos: [] });
  });
});
