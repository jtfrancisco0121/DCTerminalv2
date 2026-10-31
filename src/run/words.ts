/** Plain words for the run panel: stages, PR states, check results. */

import type { UsageTone } from "../usage/limits";
import type { LogLine, PrView, RunView } from "./api";

/**
 * The one status vocabulary of the UI. Components pick a tone from these
 * functions and the `.tone-*` classes in App.css map it to theme tokens, so
 * no component names a status colour. Every function is total: null, empty or
 * unknown input is `muted`.
 */
export type Tone = "success" | "warn" | "danger" | "running" | "muted" | "you";

export function checkTone(result: string | null | undefined): Tone {
  switch (result) {
    case "pass":
      return "success";
    case "fail":
    case "error":
    case "timeout":
      return "danger";
    case "running":
      return "running";
    case "interrupted":
      return "warn";
    default:
      return "muted";
  }
}

export function prStateTone(state: string | null | undefined): Tone {
  switch (state) {
    case "reviewed":
    case "opened":
    case "drafted":
    case "pushed":
    case "local-only":
      return "success";
    case "building":
    case "reviewing":
      return "running";
    case "fixing":
      return "warn";
    case "needs-attention":
      return "danger";
    default:
      return "muted";
  }
}

export function gateTone(kind: string | null | undefined): Tone {
  switch (kind) {
    case "proven":
      return "success";
    case "review-only":
    case "claimed":
    case "concerns":
    case "unreadable":
      return "warn";
    case "fail":
      return "danger";
    default:
      return "muted";
  }
}

/** Open findings are a warning; a verdict with none open is fine; no review yet is muted. */
export function reviewTone(review: { round?: number | null; verdict?: string | null; openFindings?: number | null } | null | undefined): Tone {
  if (!review) return "muted";
  if (review.openFindings && review.openFindings > 0) return "warn";
  if (review.verdict) return "success";
  return "muted";
}

/** The plan review badge: no open findings is fine, open ones warn, unknown is muted. */
export function planReviewTone(openFindings: number | null | undefined): Tone {
  if (openFindings == null) return "muted";
  return openFindings > 0 ? "warn" : "success";
}

/** The Risk Reviewer's verdict (plan.risk): unknown verdicts are muted. */
export function riskTone(verdict: string | null | undefined): Tone {
  switch (verdict) {
    case "NO BLOCKING RISKS":
      return "success";
    case "RISKS FOUND":
      return "warn";
    default:
      return "muted";
  }
}

export function usageTone(tone: UsageTone | null | undefined): Tone {
  switch (tone) {
    case "ok":
      return "success";
    case "warn":
      return "warn";
    case "hot":
      return "danger";
    default:
      return "muted";
  }
}

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
  { key: "ci-failed", label: "A PR's CI failed on GitHub" },
  { key: "autopost-failed", label: "A ClickUp auto-post failed" },
  { key: "merged-cleaned", label: "A merged PR was cleaned up" },
  { key: "batch-planned", label: "Every plan of a batch is ready for approval" },
  { key: "batch-ready", label: "Every run of a batch is ready to ship" },
  { key: "wave-ready", label: "An epic's next wave is ready to start" },
];

/** The events the phone gets (src-tauri/src/observe/push.rs PUSH_KINDS). */
export const PUSH_KINDS: ReadonlyArray<{ key: string; label: string }> = ATTENTION_KINDS.filter((kind) =>
  ["stall", "check-failed", "ci-failed", "merged-cleaned", "autopost-failed", "batch-planned", "batch-ready", "wave-ready"].includes(kind.key),
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

/** The GitHub CI chip of a PR: null without a PR URL or a CI reading. */
export function ciChip(pr: {
  prUrl: string | null;
  ci?: string | null;
  ciAt?: string | null;
  ciError?: string | null;
}): { text: string; tone: Tone; title: string } | null {
  if (!pr.prUrl || !pr.ci) return null;
  const read = pr.ciAt ? localTime(pr.ciAt) : null;
  const title = [pr.ciError, read ? `read ${read.short}` : null].filter(Boolean).join(" · ");
  switch (pr.ci) {
    case "pass":
      return { text: "CI pass", tone: "success", title };
    case "fail":
      return { text: "CI failed", tone: "danger", title };
    case "pending":
      return { text: "CI pending", tone: "muted", title };
    case "none":
      return { text: "CI none", tone: "muted", title };
    default:
      return { text: "CI unknown", tone: "muted", title };
  }
}

/** The app's own check result for a PR (R1), in words. */
export function appCheckText(pr: PrView): string {
  if (!pr.checkKnown) return "no check known";
  return `${checkMark(pr.appCheck?.result)} ${pr.appCheck?.result ?? "not run"}`;
}

/** The runs list: a run matches when `query` is in its title, folder, id or a ticket id (case-insensitive). */
export function runMatches(
  run: { title: string | null; folder: string | null; runId: string; ticketIds?: string[] },
  query: string,
): boolean {
  const needle = query.trim().toLowerCase();
  if (!needle) return true;
  return [run.title, run.folder, run.runId, ...(run.ticketIds ?? [])].some((text) =>
    (text ?? "").toLowerCase().includes(needle),
  );
}

/** "today", "yesterday", "3 days ago", else the date; "" for a missing or unreadable time. */
export function relativeDate(iso: string | null | undefined, now: Date = new Date()): string {
  if (!iso) return "";
  const then = new Date(iso);
  if (Number.isNaN(then.getTime())) return "";
  const day = (d: Date) => Math.floor((d.getTime() - d.getTimezoneOffset() * 60_000) / 86_400_000);
  const days = day(now) - day(then);
  if (days <= 0) return "today";
  if (days === 1) return "yesterday";
  if (days < 7) return `${days} days ago`;
  return iso.slice(0, 10);
}

/** "35 min", "2 h 5 min"; "—" with no figure. */
export function minutesText(minutes: number | null | undefined): string {
  if (minutes == null || !Number.isFinite(minutes)) return "—";
  const total = Math.round(minutes);
  if (total < 60) return `${total} min`;
  return `${Math.floor(total / 60)} h ${total % 60} min`;
}

/**
 * A time in the machine's local zone (the system zone, no hardcoded offset).
 * `short` is HH:MM (24 h, never "24:xx"); `title` is the full local date-time
 * plus the UTC ISO time, for a hover. Stored times stay UTC; only the display
 * is local. Null on an unparseable input. Built per call: no cached formatter,
 * so a changed TZ is honoured.
 */
export function localTime(at: string | number): { short: string; title: string } | null {
  const date = new Date(at);
  if (Number.isNaN(date.getTime())) return null;
  const short = new Intl.DateTimeFormat(undefined, {
    hour: "2-digit",
    minute: "2-digit",
    hourCycle: "h23",
  }).format(date);
  const full = new Intl.DateTimeFormat(undefined, {
    dateStyle: "medium",
    timeStyle: "short",
    hourCycle: "h23",
  }).format(date);
  return { short, title: `${full} (UTC ${date.toISOString().replace(/\.\d{3}Z$/, "Z")})` };
}

/** The steps of a normal run, in order (the `stage` values of status.json). */
export const RUN_STEPS: ReadonlyArray<{ key: string; label: string }> = [
  { key: "planning", label: "Planning" },
  { key: "plan-approval", label: "Plan approval" },
  { key: "building", label: "Building" },
  { key: "ship-approval", label: "Ship approval" },
  { key: "shipping", label: "Shipping" },
  { key: "done", label: "Done" },
];

/** An epic-plan run only plans: it never builds or ships (plugin/scripts/state.sh). */
export const EPIC_PLAN_STEPS: ReadonlyArray<string> = ["planning", "plan-approval", "done"];

/** The steps where the user answers. */
export const USER_STEPS: ReadonlyArray<string> = ["plan-approval", "ship-approval"];

/** PR states that count as done in `n/m PRs` (the same set as src-tauri/src/observe/attention.rs). */
export const PR_DONE_STATES: ReadonlyArray<string> = ["reviewed", "drafted", "opened", "pushed", "local-only"];

export function prsDone(prs: ReadonlyArray<{ state: string | null }>): number {
  return prs.filter((pr) => pr.state != null && PR_DONE_STATES.includes(pr.state)).length;
}

function stepSetFor(mode: string | null | undefined): ReadonlyArray<{ key: string; label: string }> {
  return mode === "epic-plan" ? RUN_STEPS.filter((step) => EPIC_PLAN_STEPS.includes(step.key)) : RUN_STEPS;
}

/** The step a failed run had reached: the stage of the last log line (oldest first) that names a step of the run's mode. */
export function reachedStep(timeline: ReadonlyArray<LogLine>, mode?: string | null): string | null {
  const keys = stepSetFor(mode).map((step) => step.key);
  for (let at = timeline.length - 1; at >= 0; at -= 1) {
    const stage = timeline[at].stage;
    if (stage && keys.includes(stage)) return stage;
  }
  return null;
}

export type StepStatus = "done" | "current" | "todo" | "waiting";
export type RunStep = { key: string; label: string; status: StepStatus; you: boolean; extra?: string };

/**
 * The stepper's steps for a run, or null for a stage the stepper does not
 * know (the stage sentence stands alone). A `failed` run shows the steps up to
 * `reached` done and the rest todo, then the failed end.
 */
export function runSteps(
  stage: string | null,
  waitingFor: string | null,
  prs: { done: number; total: number },
  opts: { mode?: string | null; reached?: string | null } = {},
): { steps: RunStep[]; failed: boolean } | null {
  if (!stage) return null;
  const set = stepSetFor(opts.mode);
  const index = (key: string | null | undefined) => (key ? set.findIndex((step) => step.key === key) : -1);
  const failed = stage === "failed";
  const at = index(stage);
  if (!failed && at < 0) return null;
  const reachedAt = failed ? index(opts.reached) : -1;
  const waitAt = failed ? -1 : index(waitingFor);
  const steps = set.map((step, i): RunStep => {
    let status: StepStatus;
    if (failed) status = i <= reachedAt ? "done" : "todo";
    else if (stage === "done") status = "done";
    else if (waitAt >= 0) status = i < waitAt ? "done" : i === waitAt ? "waiting" : "todo";
    else status = i < at ? "done" : i === at ? "current" : "todo";
    const out: RunStep = { key: step.key, label: step.label, status, you: USER_STEPS.includes(step.key) };
    if (step.key === "building" && prs.total > 0) out.extra = `${prs.done}/${prs.total} PRs`;
    return out;
  });
  return { steps, failed };
}

export type TimelineGroup = { key: string; stage: string | null; label: string; lines: LogLine[] };

/**
 * The last `limit` log lines, newest first, grouped by consecutive stage (a
 * stage that comes back forms a new group). A line with no stage is "Other".
 * The key is the stage plus the group's oldest line, so it stays the same
 * while newer lines are appended and the window shifts.
 */
export function groupTimeline(lines: ReadonlyArray<LogLine>, limit = 30): TimelineGroup[] {
  const groups: TimelineGroup[] = [];
  for (const line of lines.slice(-limit).reverse()) {
    const last = groups[groups.length - 1];
    if (last && last.stage === line.stage) {
      last.lines.push(line);
    } else {
      groups.push({ key: "", stage: line.stage, label: line.stage ? stageText(line.stage) : "Other", lines: [line] });
    }
  }
  for (const group of groups) {
    const oldest = group.lines[group.lines.length - 1];
    group.key = `${group.stage ?? ""}|${oldest.time ?? oldest.text}`;
  }
  return groups;
}
