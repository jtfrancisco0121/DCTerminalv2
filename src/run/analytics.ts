/** Pure aggregates over the Runs list (`runOverview`): the summary header, sorting and the trend series. */

import type { RunListItem } from "./api";

export const DAY_MS = 86_400_000;

export function median(values: ReadonlyArray<number>): number | null {
  const sorted = values.filter((v) => Number.isFinite(v)).sort((a, b) => a - b);
  if (sorted.length === 0) return null;
  const mid = Math.floor(sorted.length / 2);
  return sorted.length % 2 ? sorted[mid] : (sorted[mid - 1] + sorted[mid]) / 2;
}

const finished = (run: RunListItem) => run.record != null && run.record.outcome != null;

function createdMs(run: RunListItem): number | null {
  if (!run.createdAt) return null;
  const at = new Date(run.createdAt).getTime();
  return Number.isNaN(at) ? null : at;
}

export type RunsSummary = {
  /** Runs created in the window. */
  count: number;
  /** Sum of the priced estimates; null when no run in the window has a price (shown as a dash). */
  estimateUsd: number | null;
  /** Finished runs in the window without a price. */
  unpriced: number;
  /** Median active minutes of the finished runs in the window; null when there are none. */
  medianMinutes: number | null;
  days: number;
};

/** The header of the Runs table over the runs created in the last `days` days. */
export function runsSummary(runs: ReadonlyArray<RunListItem>, now: Date, days = 7): RunsSummary {
  const from = now.getTime() - days * DAY_MS;
  const inWindow = runs.filter((run) => {
    const at = createdMs(run);
    return at != null && at >= from && at <= now.getTime();
  });
  const done = inWindow.filter(finished);
  const priced = done.filter((run) => run.record?.estimateUsd != null);
  return {
    count: inWindow.length,
    estimateUsd: priced.length ? priced.reduce((sum, run) => sum + (run.record?.estimateUsd ?? 0), 0) : null,
    unpriced: done.length - priced.length,
    medianMinutes: median(done.map((run) => run.record?.minutes ?? Number.NaN)),
    days,
  };
}

export type SortKey = "duration" | "estimate";
export type SortDir = "asc" | "desc";

/** A new array; with no key, newest first. Runs lacking the value come last in both directions. */
export function sortRuns(runs: ReadonlyArray<RunListItem>, key: SortKey | null, dir: SortDir = "desc"): RunListItem[] {
  const value = (run: RunListItem): number | null =>
    key === "duration" ? (run.record?.minutes ?? null) : key === "estimate" ? (run.record?.estimateUsd ?? null) : createdMs(run);
  const sign = key === null ? -1 : dir === "asc" ? 1 : -1;
  return runs
    .map((run, at) => ({ run, at, v: value(run) }))
    .sort((a, b) => {
      if (a.v == null && b.v == null) return a.at - b.at;
      if (a.v == null) return 1;
      if (b.v == null) return -1;
      return a.v === b.v ? a.at - b.at : sign * (a.v - b.v);
    })
    .map((entry) => entry.run);
}

export type TrendFilters = { folder?: string; repo?: string };
export type TrendPoint = { runId: string; at: string; label: string; value: number };
export type Trend = {
  estimate: TrendPoint[];
  minutes: TrendPoint[];
  /** Wait over span, 0 to 1. */
  waitShare: TrendPoint[];
  /** Finished runs left out of each series (no price, no figure, span 0). */
  skipped: { estimate: number; minutes: number; waitShare: number };
};

/** Finished runs, oldest first, as three series; runs without a figure are counted in `skipped`. */
export function trendSeries(runs: ReadonlyArray<RunListItem>, filters: TrendFilters = {}): Trend {
  const picked = runs
    .filter(finished)
    .filter((run) => !filters.folder || run.folder === filters.folder)
    .filter((run) => !filters.repo || (run.repos ?? []).includes(filters.repo))
    .filter((run) => createdMs(run) != null)
    .sort((a, b) => (createdMs(a) ?? 0) - (createdMs(b) ?? 0));
  const out: Trend = { estimate: [], minutes: [], waitShare: [], skipped: { estimate: 0, minutes: 0, waitShare: 0 } };
  for (const run of picked) {
    const record = run.record;
    const point = (value: number): TrendPoint => ({
      runId: run.runId,
      at: run.createdAt ?? "",
      label: run.title ?? run.runId,
      value,
    });
    if (record?.estimateUsd != null) out.estimate.push(point(record.estimateUsd));
    else out.skipped.estimate += 1;
    if (record && Number.isFinite(record.minutes)) out.minutes.push(point(record.minutes));
    else out.skipped.minutes += 1;
    const wait = record?.waitMinutes;
    const span = record?.spanMinutes;
    if (wait != null && span != null && span > 0) out.waitShare.push(point(Math.min(1, Math.max(0, wait / span))));
    else out.skipped.waitShare += 1;
  }
  return out;
}

/** The distinct folders and repo names to filter by, sorted. */
export function filterOptions(runs: ReadonlyArray<RunListItem>): { folders: string[]; repos: string[] } {
  const folders = new Set<string>();
  const repos = new Set<string>();
  for (const run of runs) {
    if (run.folder) folders.add(run.folder);
    for (const repo of run.repos ?? []) if (repo) repos.add(repo);
  }
  return { folders: [...folders].sort(), repos: [...repos].sort() };
}
