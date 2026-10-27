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

/** The event kinds the detector announces (src-tauri/src/observe/attention.rs), for Settings. */
export const ATTENTION_KINDS: ReadonlyArray<{ key: string; label: string }> = [
  { key: "needs-you", label: "A Claude tab needs you" },
  { key: "approval-wait", label: "A run waits for your approval" },
  { key: "waiting-reminder", label: "Reminder: still waiting for you" },
  { key: "stall", label: "A run has made no progress" },
  { key: "run-done", label: "A run finished" },
  { key: "run-failed", label: "A run failed" },
  { key: "check-failed", label: "A PR's check failed" },
  { key: "autopost-failed", label: "A ClickUp auto-post failed" },
  { key: "merged-cleaned", label: "A merged PR was cleaned up" },
];

/** The events the phone gets (src-tauri/src/observe/push.rs PUSH_KINDS). */
export const PUSH_KINDS: ReadonlyArray<{ key: string; label: string }> = ATTENTION_KINDS.filter((kind) =>
  ["stall", "check-failed", "merged-cleaned", "autopost-failed"].includes(kind.key),
);

const PR_STATES: Record<string, string> = {
  pending: "waiting",
  building: "building",
  reviewing: "in review",
  fixing: "fixing review findings",
  reviewed: "reviewed",
  "needs-attention": "needs attention",
  drafted: "draft PR open",
  opened: "PR opened",
  pushed: "pushed (PR not opened)",
  "local-only": "local only",
};

export function prStateText(state: string | null): string {
  if (!state) return "unknown";
  return PR_STATES[state] ?? state;
}

/** Review on GitHub: "draft" while the PR is a draft, "ready" once marked ready; null when not on GitHub. */
export function draftBadge(pr: { draft?: boolean | null; prUrl: string | null; state: string | null }): string | null {
  if (pr.draft === true || pr.state === "drafted") return "draft";
  if (pr.draft === false && pr.prUrl) return "ready";
  return null;
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
    case "interrupted":
      return "?";
    default:
      return "–";
  }
}

/** The run is over: offer the worktree clean-up. */
export function runFinished(view: RunView): boolean {
  return view.stage === "done" || view.stage === "failed";
}

/**
 * R1 should run on its own: at stop #2, for a PR with a known check
 * (repos.json) that has no app result yet or whose HEAD moved since.
 */
export function wantsAppChecks(view: RunView): boolean {
  return (
    view.stage === "ship-approval" &&
    view.prs.some((pr) => pr.checkKnown && (!pr.appCheck || pr.appCheckStale))
  );
}

/** What the auto re-run was last done for: each PR's HEAD. */
export function checkSignature(view: RunView): string {
  return view.prs.map((pr) => `${pr.id}@${pr.currentHead ?? "-"}`).join(" ");
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

/** The approval stops (status.json `waitingFor`) and their titles. */
const APPROVALS: Record<string, string> = {
  "plan-approval": "Plan approval",
  "ship-approval": "Ship approval",
};

/** "Plan approval" / "Ship approval"; an unknown value is shown as it is. */
export function approvalTitle(waitingFor: string): string {
  return APPROVALS[waitingFor] ?? waitingFor;
}

export function isKnownApproval(waitingFor: string): boolean {
  return waitingFor in APPROVALS;
}

/** The app's own check result for a PR (R1), in words. */
export function appCheckText(pr: PrView): string {
  if (!pr.checkKnown) return "no check known";
  return `${checkMark(pr.appCheck?.result)} ${pr.appCheck?.result ?? "not run"}`;
}
