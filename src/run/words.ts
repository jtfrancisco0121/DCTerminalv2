/** Plain words for the run panel: stages, PR states, check results. */

import type { PrView, RunView } from "./api";

const STAGES: Record<string, string> = {
  planning: "Planning",
  "plan-approval": "Waiting for your plan approval",
  building: "Building",
  "ship-approval": "Waiting for your go to open the PRs",
  shipping: "Opening PRs",
  done: "Done",
  failed: "Failed",
};

/** Unknown stages are shown as they are. */
export function stageText(stage: string | null): string {
  if (!stage) return "Starting";
  return STAGES[stage] ?? stage;
}

const PR_STATES: Record<string, string> = {
  pending: "waiting",
  building: "building",
  reviewing: "in review",
  fixing: "fixing review findings",
  reviewed: "reviewed",
  "needs-attention": "needs attention",
  opened: "PR opened",
  pushed: "pushed (PR not opened)",
  "local-only": "local only",
};

export function prStateText(state: string | null): string {
  if (!state) return "unknown";
  return PR_STATES[state] ?? state;
}

export function checkMark(result: string | null | undefined): string {
  switch (result) {
    case "pass":
      return "✓";
    case "fail":
    case "error":
    case "timeout":
      return "✗";
    case "running":
      return "…";
    default:
      return "–";
  }
}

/** The run is over: offer the worktree clean-up. */
export function runFinished(view: RunView): boolean {
  return view.stage === "done" || view.stage === "failed";
}

/** R1 should run on its own: at stop #2 and no app check result yet. */
export function wantsAppChecks(view: RunView): boolean {
  return view.stage === "ship-approval" && view.prs.length > 0 && view.prs.every((pr) => !pr.appCheck);
}

export function storiesText(pr: PrView): string | null {
  if (pr.storiesTotal == null && pr.storiesDone == null) return null;
  return `${pr.storiesDone ?? 0}/${pr.storiesTotal ?? "?"} stories`;
}

export function reviewText(pr: PrView): string | null {
  const { round, verdict, openFindings } = pr.review;
  if (!round && !verdict) return null;
  const parts = [`review round ${round ?? 0}`];
  if (verdict) parts.push(verdict);
  if (openFindings) parts.push(`${openFindings} open`);
  return parts.join(", ");
}

/** A link the opener may open (http/https only). */
export function safeUrl(url: string | null): string | null {
  return url && /^https?:\/\//i.test(url) ? url : null;
}
