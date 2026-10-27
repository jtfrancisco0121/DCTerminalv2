/** A cost range for a run about to start, from earlier finished runs like it. */

import type { RunListItem } from "./api";

/** How many of the newest finished runs are looked at. */
const WINDOW = 20;
/** Fewer matching runs than this is not an estimate. */
const MIN_RUNS = 3;

export type Estimate = { low: number; high: number; n: number };

/** 1, 2-3 or 4+ repos. */
function bucket(repos: number): number {
  return repos <= 1 ? 1 : repos <= 3 ? 2 : 3;
}

/**
 * The range of `record.estimateUsd` over the newest 20 finished (done or
 * failed) runs that have an estimate, of those with the same repo bucket and the same BMAD flag. `bmadExpected`
 * null means "auto": both kinds count. Null with fewer than 3 such runs.
 * `runs` is newest first (the order the runs list comes in).
 */
export function estimate(runs: readonly RunListItem[], repoCount: number, bmadExpected: boolean | null): Estimate | null {
  const wanted = bucket(repoCount);
  const costs = runs
    .filter((run) => run.stage === "done" || run.stage === "failed")
    .filter((run) => typeof run.record?.estimateUsd === "number" && run.repoCount != null)
    .slice(0, WINDOW)
    .filter((run) => bucket(run.repoCount ?? 1) === wanted)
    .filter((run) => bmadExpected === null || Boolean(run.bmadApplied) === bmadExpected)
    .map((run) => run.record!.estimateUsd as number);
  if (costs.length < MIN_RUNS) return null;
  return { low: Math.min(...costs), high: Math.max(...costs), n: costs.length };
}

/** "Similar runs: $4–$9 (6 runs)". */
export function estimateText(found: Estimate | null): string {
  if (!found) return "Not enough earlier runs for an estimate.";
  const dollars = (usd: number) => `$${Math.round(usd)}`;
  return `Similar runs: ${dollars(found.low)}–${dollars(found.high)} (${found.n} runs)`;
}
