/** Plain words for the run panel: stages, PR states, check results. */

import { KIND, STATUS } from "../flight/types";
import type { UsageTone } from "../usage/limits";
import type { EvalRef, LogLine, PrView, RunView } from "./api";

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
    case "qa":
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

/** The words for a finished eval run, wherever its stage or outcome is shown. */
export const EVAL_FINISHED_TEXT = "Eval run finished";

/**
 * A run of an eval group that has ended: it carries meta.json `eval` and stops at `ship-approval` for good (nothing is
 * shipped). The twin of Rust `folder::finished_eval`; every function below that reads a run's stage checks it first.
 */
export function evalFinished(run: { stage?: string | null; eval?: EvalRef | null } | null | undefined): boolean {
  return Boolean(run && run.eval && run.stage === "ship-approval");
}

/**
 * Unknown stages are shown as they are. Pass the run's `eval` where there is one: a finished eval run reads "Eval run
 * finished", never "Waiting for your go to open the PRs". Without the second argument it is unchanged.
 */
export function stageText(stage: string | null, evalRef?: EvalRef | null): string {
  if (!stage) return "Starting";
  if (evalRef && stage === "ship-approval") return EVAL_FINISHED_TEXT;
  return STAGES[stage] ?? stage;
}

/** The words of a run record's outcome: `eval-done` is "Eval run finished", the rest as a stage. */
export function outcomeText(outcome: string | null | undefined): string {
  if (outcome === "eval-done") return EVAL_FINISHED_TEXT;
  return stageText(outcome ?? null);
}

/** The event kinds the detector announces (src-tauri/src/observe/attention.rs), for Settings. */
export const ATTENTION_KINDS: ReadonlyArray<{ key: string; label: string }> = [
  { key: "needs-you", label: "A Claude tab needs you" },
  { key: "approval-wait", label: "A run waits for your approval" },
  { key: "waiting-reminder", label: "Reminder: still waiting for you" },
  { key: "stall", label: "A run has made no progress" },
  { key: "not-started", label: "A run tab was created but did not start" },
  { key: "run-done", label: "A run finished" },
  { key: "run-failed", label: "A run failed" },
  { key: "check-failed", label: "A PR's check failed" },
  { key: "ci-failed", label: "A PR's CI failed on GitHub" },
  { key: "autopost-failed", label: "A ClickUp auto-post failed" },
  { key: "merged-cleaned", label: "A merged PR was cleaned up" },
  { key: "batch-planned", label: "Every plan of a batch is ready for approval" },
  { key: "batch-ready", label: "Every run of a batch is ready to ship" },
  { key: "wave-ready", label: "An epic's next wave is ready to start" },
  { key: "eval-finished", label: "Every run of an eval group has finished" },
];

/** The events the phone gets (src-tauri/src/observe/push.rs PUSH_KINDS). */
export const PUSH_KINDS: ReadonlyArray<{ key: string; label: string }> = ATTENTION_KINDS.filter((kind) =>
  ["stall", "not-started", "check-failed", "ci-failed", "merged-cleaned", "autopost-failed", "batch-planned", "batch-ready", "wave-ready", "eval-finished"].includes(kind.key),
);

const PR_STATES: Record<string, string> = {
  pending: "waiting",
  building: "building",
  reviewing: "in review",
  qa: "trying in the app",
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

/** The run is over: offer the worktree clean-up. A finished eval run is over too. */
export function runFinished(view: { stage?: string | null; eval?: EvalRef | null }): boolean {
  return view.stage === "done" || view.stage === "failed" || evalFinished(view);
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

/**
 * The QA pill of a PR: "QA trying…" while it is in state `qa` with no result yet, then "QA n/m passed", "QA failed" or
 * "QA could not run". null when QA did not apply (no result and not in state `qa`).
 */
export function qaPill(pr: Pick<PrView, "state" | "qa">): { text: string; tone: Tone } | null {
  const qa = pr.qa;
  if (!qa) return pr.state === "qa" ? { text: "QA trying…", tone: "running" } : null;
  if (qa.result === "pass") return { text: `QA ${Math.max(0, qa.tried - qa.failed)}/${qa.total} passed`, tone: "success" };
  if (qa.result === "fail") return { text: "QA failed", tone: "danger" };
  if (qa.result === "error") return { text: "QA could not run", tone: "warn" };
  return null;
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
/** A triage run only proposes: it plans, then is done (there is no stop #1 or #2). */
export const TRIAGE_STEPS: ReadonlyArray<string> = ["planning", "done"];

/**
 * An eval run plans, builds and reviews, then ends at `ship-approval` with nothing to approve (state.sh refuses the
 * ship steps): its last step is the end, and its plan approval is not the user's.
 */
export const EVAL_STEPS: ReadonlyArray<{ key: string; label: string }> = [
  { key: "planning", label: "Planning" },
  { key: "plan-approval", label: "Plan approval" },
  { key: "building", label: "Building" },
  { key: "ship-approval", label: "Eval finished" },
];

/** The steps where the user answers. */
export const USER_STEPS: ReadonlyArray<string> = ["plan-approval", "ship-approval"];

/** PR states that count as done in `n/m PRs` (the same set as src-tauri/src/observe/attention.rs). */
export const PR_DONE_STATES: ReadonlyArray<string> = ["reviewed", "drafted", "opened", "pushed", "local-only"];

/** "1 PR", "2 PRs", "0 PRs": the one PR plural rule of the UI. */
export function prCount(n: number): string {
  return `${n} ${n === 1 ? "PR" : "PRs"}`;
}

/** "0/1 PR", "1/2 PRs": PRs done of total, with the same plural rule. */
export function prFraction(done: number, total: number): string {
  return `${done}/${total} ${total === 1 ? "PR" : "PRs"}`;
}

export function prsDone(prs: ReadonlyArray<{ state: string | null }>): number {
  return prs.filter((pr) => pr.state != null && PR_DONE_STATES.includes(pr.state)).length;
}

function stepSetFor(mode: string | null | undefined): ReadonlyArray<{ key: string; label: string }> {
  if (mode === "eval") return EVAL_STEPS;
  if (mode === "triage") return RUN_STEPS.filter((step) => TRIAGE_STEPS.includes(step.key));
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
  opts: { mode?: string | null; reached?: string | null; finished?: boolean } = {},
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
    else if (stage === "done" || opts.finished) status = "done";
    else if (waitAt >= 0) status = i < waitAt ? "done" : i === waitAt ? "waiting" : "todo";
    else status = i < at ? "done" : i === at ? "current" : "todo";
    const you = opts.mode !== "eval" && USER_STEPS.includes(step.key);
    const out: RunStep = { key: step.key, label: step.label, status, you };
    if (step.key === "building" && prs.total > 0) out.extra = prFraction(prs.done, prs.total);
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
export function groupTimeline(lines: ReadonlyArray<LogLine>, limit = 30, evalRef?: EvalRef | null): TimelineGroup[] {
  const groups: TimelineGroup[] = [];
  for (const line of lines.slice(-limit).reverse()) {
    const last = groups[groups.length - 1];
    if (last && last.stage === line.stage) {
      last.lines.push(line);
    } else {
      groups.push({ key: "", stage: line.stage, label: line.stage ? stageText(line.stage, evalRef) : "Other", lines: [line] });
    }
  }
  for (const group of groups) {
    const oldest = group.lines[group.lines.length - 1];
    group.key = `${group.stage ?? ""}|${oldest.time ?? oldest.text}`;
  }
  return groups;
}

/** The note on every cost figure: it is a list-price estimate, not a bill. */
export const ESTIMATE_NOTE = "An estimate at API list prices, not a bill";
/** The caption of the cost chart. */
export const ESTIMATES_CAPTION = "Estimates at API list prices";

/** What `tabProgress` reads of a run tab (the `ActiveRun` of the bridge, structurally). */
export type TabReading = {
  runId?: string | null;
  stage?: string | null;
  waitingFor?: string | null;
  runMode?: string | null;
  eval?: EvalRef | null;
  prsDone: number;
  prsTotal: number;
};

export type TabProgress = {
  /** 0 to 1: the width of the tab underline. */
  fraction: number;
  tone: Tone;
  /** The run waits for the person. */
  waiting: boolean;
  /** For the tab's tooltip and accessible name. */
  title: string;
};

/**
 * The progress of a run tab, or null for a tab without a run or a run without a stage.
 * With PRs the underline is PRs done of total, else the share of steps reached.
 */
export function tabProgress(reading: TabReading | null | undefined): TabProgress | null {
  if (!reading || !reading.runId || !reading.stage) return null;
  const { stage, prsDone: done, prsTotal: total } = reading;
  const prs = total > 0 ? prFraction(done, total) : null;
  const join = (head: string) => (prs ? `${head} · ${prs}` : head);
  if (stage === "failed") return { fraction: 1, tone: "danger", waiting: false, title: join("Failed") };
  if (stage === "done") return { fraction: 1, tone: "success", waiting: false, title: join("Done") };
  // A finished eval run is over, not waiting for anyone.
  if (evalFinished(reading)) return { fraction: 1, tone: "success", waiting: false, title: join(EVAL_FINISHED_TEXT) };
  const waiting = Boolean(reading.waitingFor);
  const set = stepSetFor(reading.runMode);
  const at = set.findIndex((step) => step.key === stage);
  const fraction = total > 0 ? Math.min(1, done / total) : at >= 0 ? at / (set.length - 1) : 0;
  return {
    fraction,
    tone: waiting ? "you" : "running",
    waiting,
    title: join(waiting ? "Waiting for you" : stageText(stage)),
  };
}

/** The tone of a run's outcome in the Runs table: the record's outcome, else the stage. */
export function outcomeTone(
  outcome: string | null | undefined,
  stage: string | null | undefined,
  evalRef?: EvalRef | null,
): Tone {
  const value = outcome ?? stage;
  if (value === "eval-done" || evalFinished({ stage, eval: evalRef })) return "success";
  if (value === "done") return "success";
  if (value === "failed") return "danger";
  return stage ? "running" : "muted";
}

/** A trace event's status: ok, error, blocked (a hook or guard said no), cancelled; null is a call still open. */
export function flightStatusTone(status: string | null | undefined): Tone {
  switch (status) {
    case "ok":
      return "success";
    case "error":
      return "danger";
    case "blocked":
      return "warn";
    default:
      return "muted";
  }
}

export function flightStatusText(status: string | null | undefined): string {
  switch (status) {
    case "ok":
      return "ok";
    case "error":
      return "failed";
    case "blocked":
      return "blocked";
    case "cancelled":
      return "cancelled";
    default:
      return "open";
  }
}

const FLIGHT_KINDS: Record<string, string> = {
  "session.start": "Session started",
  "turn.start": "Prompt",
  "turn.end": "Turn ended",
  "llm.call": "Model call",
  "tool.start": "Tool call",
  "tool.end": "Tool result",
  "tool.blocked": "Tool blocked",
  "agent.spawn": "Agent started",
  "file.change": "File change",
  error: "API error",
  "x.claude.unknown": "Unknown line",
};

/** Plain words for an event kind; an unknown kind is shown as it is. */
export function flightKindText(kind: string): string {
  return FLIGHT_KINDS[kind] ?? kind;
}

/** "exact:is_error" is a fact; "heuristic:…" is a guess and is labelled so. */
export function flightDerivedText(label: string): string {
  return label.startsWith("heuristic:") ? "heuristic" : "exact";
}

/** What the orchestration words need of an event. */
export interface OrchestrationEvent {
  kind: string;
  name: string | null;
  status: string | null;
  attrs: Record<string, unknown>;
}

/**
 * The tone of an event of the run folder's own lane: danger for a failed check, stop-check or CI; warn for a guard
 * block, a retry and a declined approval; success for a pass, an approval and a passing CI; running for a stage; you for
 * an approval wait; muted for the plugin's own `x.dcv2.*` rows and a skipped check.
 */
export function orchestrationTone(kind: string, status: string | null | undefined): Tone {
  switch (kind) {
    case KIND.CHECK_END:
      return status === STATUS.PASS ? "success" : status === STATUS.FAIL || status === "error" ? "danger" : "muted";
    case KIND.CI_STATE:
      return status === STATUS.PASS ? "success" : status === STATUS.FAIL ? "danger" : "muted";
    case KIND.TOOL_BLOCKED:
    case KIND.RETRY:
    case KIND.ATTENTION:
      return "warn";
    case KIND.STAGE_CHANGE:
      return "running";
    case KIND.APPROVAL_WAIT:
      return "you";
    case KIND.APPROVAL_DECISION:
      return status === STATUS.OK ? "success" : "warn";
    default:
      return "muted";
  }
}

const str = (v: unknown): string => (typeof v === "string" ? v : "");
const capital = (s: string) => (s ? s[0]!.toUpperCase() + s.slice(1) : s);
const CHECK_WORD: Record<string, string> = { pass: "passed", fail: "failed", skip: "skipped", error: "could not run" };

/** The words of a row of the run's own lane. No log text: the text is read on demand, redacted. */
export function orchestrationText(e: OrchestrationEvent): string {
  const pr = str(e.attrs.pr);
  switch (e.kind) {
    case KIND.STAGE_CHANGE:
      return `Stage: ${e.name ?? "?"}`;
    case KIND.APPROVAL_WAIT:
      return `Waiting for ${e.name ?? ""} approval`.replace("  ", " ");
    case KIND.APPROVAL_DECISION:
      return `${capital(e.name ?? "")} ${e.status === STATUS.OK ? "approved" : "declined"}`.trim();
    case KIND.REVIEW_ROUND: {
      const who = pr || capital((e.name ?? "").split("-round")[0] ?? "");
      return `${who} review round ${String(e.attrs.round ?? "?")}`.trim();
    }
    case KIND.CHECK_END: {
      const word = CHECK_WORD[e.status ?? ""] ?? "finished";
      const source = str(e.attrs.source);
      if (source === "stop-check") return `stop-check ${pr ? `${pr} ` : ""}${word}`;
      return `Check (${source || "run"}) ${pr ? `${pr} ` : ""}${word}`;
    }
    case KIND.CI_STATE:
      return `CI ${pr ? `${pr} ` : ""}${CHECK_WORD[str(e.attrs.ci)] ?? (str(e.attrs.ci) || "unknown")}`;
    case KIND.PR_STATE:
      return `pr ${pr} ${str(e.attrs.from) || "-"} → ${str(e.attrs.to) || e.name || "?"}`;
    case KIND.RETRY:
      return `Retry${pr ? ` ${pr}` : ""}`;
    case KIND.ATTENTION:
      return e.name ? `Needs attention: ${e.name}` : "Needs attention";
    case KIND.LOG:
      return "Log line";
    case KIND.REQUEST:
      return `Request: ${e.name ?? "?"}${str(e.attrs.result) ? ` ${str(e.attrs.result)}` : ""}`;
    case KIND.TOOL_BLOCKED:
      return `Guard blocked ${e.name ?? "a tool"}`;
    default:
      return e.kind;
  }
}

/** The word on an orchestration row's pill. */
export function orchestrationStatusText(e: OrchestrationEvent): string | null {
  if (e.kind === KIND.TOOL_BLOCKED) return "blocked";
  if (e.kind === KIND.APPROVAL_DECISION) return e.status === STATUS.OK ? "approved" : "declined";
  if (e.kind === KIND.CHECK_END || e.kind === KIND.CI_STATE) return CHECK_WORD[e.status ?? ""] ?? null;
  return null;
}

/**
 * The reason of the run's latest "trace snapshot failed" log line (written by the app's index thread), or null once a
 * later "trace snapshot written" line says it was made. Lines read `app: trace snapshot failed: <reason>`.
 */
export function snapshotFailure(timeline: ReadonlyArray<{ text: string }>): string | null {
  for (let at = timeline.length - 1; at >= 0; at -= 1) {
    const text = timeline[at]!.text;
    const failed = /^app: trace snapshot failed: (.*)$/.exec(text);
    if (failed) return failed[1] ?? "";
    if (/^app: trace snapshot written$/.test(text)) return null;
  }
  return null;
}

// ---- Compare (flight/compare.rs): the words of the rows and of the first difference ----------

const COMPARE_GROUP_TEXT: Record<string, string> = {
  stages: "Active minutes per stage",
  tools: "Tool calls",
  failures: "Failures",
  retries: "Retries",
  repeats: "Repeats",
  files: "Files touched",
  checks: "Checks",
  tokens: "Tokens",
  windowShare: "Window share",
};

/** The heading of a Compare row group. */
export function compareGroupText(group: string): string {
  return COMPARE_GROUP_TEXT[group] ?? group;
}

const COMPARE_KEY_TEXT: Record<string, string> = {
  input: "input",
  output: "output",
  cacheWrite: "cache write",
  cacheRead: "cache read",
  fiveHour: "five-hour window",
  weekly: "weekly window",
  "tool.end": "failed tool calls",
  "tool.blocked": "guard blocks",
  error: "errors",
  "check.end": "failed checks",
};

/** The words of a row's key: a guard block with its rule, a check with its source and PR, a stage as it is. */
export function compareKeyText(group: string, key: string): string {
  if (group === "failures") {
    const [kind, rule] = [key.split(":")[0] ?? key, key.includes(":") ? key.slice(key.indexOf(":") + 1) : null];
    const base = COMPARE_KEY_TEXT[kind] ?? kind;
    return rule ? `${base} (${rule})` : base;
  }
  if (group === "checks") {
    const [source, pr] = key.split("/");
    return `${source ?? key} check${pr && pr !== "-" ? `, ${pr}` : ""}`;
  }
  if (group === "retries") return key === "-" ? "retries (no PR)" : `retries of ${key}`;
  return COMPARE_KEY_TEXT[key] ?? key;
}

/** A cell: null is "not recorded"; minutes, percent points (an estimate) and counts read as what they are. */
export function compareValueText(group: string, value: number | string | null | undefined): string {
  if (value === null || value === undefined) return "not recorded";
  if (typeof value === "string") return value;
  if (group === "stages") return `${value} min`;
  if (group === "windowShare") return `${value} points (estimate)`;
  if (group === "tokens") return value.toLocaleString("en-US");
  return String(value);
}

/** "First difference: guard blocks (run-file-write) (0 vs 1)" for the Divergence row, or a no-difference line. */
export function compareSentence(
  row: { group: string; key: string; a: number | string | null; b: number | string | null } | null | undefined,
): string {
  if (!row) return "No differences in these rows.";
  return `First difference: ${compareGroupText(row.group)}, ${compareKeyText(row.group, row.key)} (${compareValueText(row.group, row.a)} vs ${compareValueText(row.group, row.b)})`;
}

/** Whether a Compare cell can open a Trace (record numbers cannot). */
export function compareOpensTrace(group: string): boolean {
  return group !== "tokens" && group !== "windowShare" && group !== "stages";
}


// ---- Eval (AF14): the usage hold, the estimate and the distribution texts -------------------

/** The latest five-hour reading of an account: percent used, and when that window resets (Unix seconds). */
export type FiveHourReading = { percent: number; resetsAt: number | null };

/**
 * Why an eval start is held now, or null. The twin of dispatch.rs `eval_hold`: with a hold set and no reading at all
 * it holds ("not known"); a reading past its reset is clear (the window has reset); otherwise the hold applies at or
 * past its percent. With no hold set it is clear.
 */
export function evalHoldReason(
  hold: number | null | undefined,
  reading: FiveHourReading | null | undefined,
  nowSec: number,
): string | null {
  if (hold === null || hold === undefined) return null;
  if (!reading) return "Five-hour usage is not known: Start waits until a Claude tab reports it.";
  if (reading.resetsAt !== null && reading.resetsAt <= nowSec) return null;
  if (reading.percent >= hold) {
    return `Five-hour usage is ${Math.round(reading.percent)}%, at or past your hold of ${hold}%.`;
  }
  return null;
}

/** The usage line of the Eval dialog: the current five-hour percent and the hold, or why neither is known. */
export function evalUsageLine(
  hold: number | null | undefined,
  reading: FiveHourReading | null | undefined,
  nowSec: number,
): string {
  if (hold === null || hold === undefined) return "No usage hold is set";
  if (!reading) return "Five-hour usage is not known";
  if (reading.resetsAt !== null && reading.resetsAt <= nowSec) return `The five-hour window has reset (hold ${hold}%)`;
  return `Five-hour usage ${Math.round(reading.percent)}% · hold ${hold}%`;
}

/** N times the source run's Window share (percent points of the five-hour window), or that it is not known. */
export function evalEstimateText(n: number, share: number | null | undefined): string {
  if (share === null || share === undefined || !Number.isFinite(share)) return "Estimate: not known";
  const total = Math.round(n * share * 10) / 10;
  return `Estimate: about ${total} points of the five-hour window (${n} × ${Math.round(share * 10) / 10})`;
}

/** The base line of the Eval dialog. */
export function evalBaseText(based: ReadonlyArray<string>, fallback: ReadonlyArray<string>): string {
  const name = (path: string) => path.split(/[\\/]/).filter(Boolean).pop() ?? path;
  if (based.length === 0) return "Builds on the current default branch";
  if (fallback.length === 0) return "Builds on the source run's commits";
  return `Builds on the source run's commits, and on the current default branch for ${fallback.map(name).join(", ")}`;
}

/** What a check state reads as in the Eval results' Checks column. */
export function evalCheckText(state: string, passRate: number | null | undefined): string {
  if (state === "checking") return "checking…";
  if (passRate === null || passRate === undefined) return "not checked";
  return `${Math.round(passRate * 100)}% pass`;
}

/** "min 1 · median 2 · max 4 (over 3 runs)" for one measure; "no figures" when no finished run has it. */
export function evalDistText(
  dist: { min: number | null; median: number | null; max: number | null; of: number },
  format: (value: number) => string = (v) => String(Math.round(v * 100) / 100),
): string {
  if (dist.min === null || dist.median === null || dist.max === null) return "no figures";
  return `min ${format(dist.min)} · median ${format(dist.median)} · max ${format(dist.max)} (over ${dist.of} ${dist.of === 1 ? "run" : "runs"})`;
}

/** Phoenix's local OTLP/HTTP traces address (the host default, `settings_store.rs`). */
export const DEFAULT_EXPORT_ENDPOINT = "http://127.0.0.1:6006/v1/traces";

/** Export refusals the host words (the host and this file hold the same text). */
export const EXPORT_CHANGED = "the trace changed since the preview";
export const EXPORT_TOO_LARGE = "This trace is larger than 32 MiB; export a single session instead.";

/** "412 spans, 38 events, 61.2 KiB". */
export function exportSizeText(spans: number, events: number, bytes: number): string {
  const size = bytes >= 1024 * 1024 ? `${(bytes / 1024 / 1024).toFixed(1)} MiB` : `${(bytes / 1024).toFixed(1)} KiB`;
  return `${spans} spans, ${events} events, ${size}`;
}

/** The warning above Send when the endpoint is not on this machine. */
export function exportLeavesText(spans: number, endpoint: string): string {
  return `This export leaves the machine: ${spans} spans to ${endpoint}`;
}

/** The labels of replayed output: what the agent did, what the app ran again, and what a re-run starts. */
export const REPLAY_LABEL = { recorded: "recorded", rerun: "re-run", fresh: "new execution" } as const;

/** What replay cannot see (the file pane's footnote). */
export const REPLAY_FOOTNOTE = "Changes made by shell commands are not seen.";

/** Why the replay toggle is disabled, or null. */
export function replayDisabledReason(eventCount: number, off: boolean): string | null {
  if (off) return "AgentFlight is off for this account (Settings › Activity).";
  if (eventCount === 0) return "This trace has no events to replay.";
  return null;
}

/** "after edit 3 of 7". */
export const replayEditText = (k: number, n: number) => `after edit ${k} of ${n}`;

/** The re-run states, in words and tone (muted when it did not finish, running while it runs). */
export function rerunStateText(state: string): string {
  switch (state) {
    case "preparing":
      return "preparing";
    case "setup":
      return "setting up";
    case "running":
      return "running the check";
    case "pass":
      return "pass";
    case "fail":
      return "fail";
    case "timeout":
      return "timed out";
    case "error":
      return "error";
    case "stopped":
      return "stopped";
    case "interrupted":
      return "interrupted (the app quit)";
    default:
      return state;
  }
}

export function rerunStateTone(state: string): Tone {
  switch (state) {
    case "preparing":
    case "setup":
    case "running":
      return "running";
    case "pass":
      return "success";
    case "fail":
    case "timeout":
    case "error":
      return "danger";
    default:
      return "muted";
  }
}

/** A re-run that is still going. */
export const rerunActive = (state: string) => state === "preparing" || state === "setup" || state === "running";

/** The disabled "Re-run check" caption for a check that is not the app's own. */
export const RERUN_NOT_APP = "Only the app's own check can be re-run.";

/** The note in the confirmation when the recorded check ran on a dirty worktree. */
export const RERUN_DIRTY_NOTE =
  "The recorded check ran with uncommitted changes in the worktree; the re-run uses the committed files at this commit only.";

/** "Re-run PR2's check at 3f2a9c1". */
export const rerunTitle = (pr: string, commit: string | null) => `Re-run ${pr}'s check${commit ? ` at ${commit.slice(0, 7)}` : ""}`;

/** The last `n` lines of a log. */
export function lastLines(text: string, n: number): string {
  const lines = text.replace(/\n$/, "").split("\n");
  return lines.slice(-n).join("\n");
}

/** A web QA charter row's result (PASS, FAIL or ERROR). */
export function webqaResultTone(result: string | null | undefined): Tone {
  switch (result) {
    case "PASS":
    case "pass":
      return "success";
    case "FAIL":
    case "fail":
      return "danger";
    case "ERROR":
    case "error":
      return "warn";
    default:
      return "muted";
  }
}

/** The badge that compares a row or a finding with the pass before it; null hides it. */
export function comparisonBadge(comparison: string | null | undefined): { text: string; tone: Tone } | null {
  switch (comparison) {
    case "new":
      return { text: "New", tone: "warn" };
    case "fixed":
      return { text: "Fixed", tone: "success" };
    case "still-failing":
      return { text: "Still failing", tone: "danger" };
    default:
      return null;
  }
}

// ---- The run panel's folding sections (14yq6g325n9) ----

/** The sections of the run panel that fold, in the order they appear. */
export type FoldId = "session" | "plan" | "prs" | "cleanup" | "agents" | "clickup" | "timeline";

export const FOLD_TITLES: Record<FoldId, string> = {
  session: "Session",
  plan: "Plan",
  prs: "PRs",
  cleanup: "Cleanup",
  agents: "Agents",
  clickup: "ClickUp",
  timeline: "Timeline",
};

/** What a folded section still says in one line, and the tone of what it holds. */
export type FoldInfo = { summary: string; tone: Tone };

/** A section holding something for the user, or something wrong, cannot be folded away. */
export function foldForcedOpen(tone: Tone): boolean {
  return tone === "you" || tone === "warn" || tone === "danger";
}

const TONE_RANK: Record<Tone, number> = { muted: 0, success: 1, running: 2, warn: 3, you: 4, danger: 5 };

/** The tone that needs the most attention; `muted` for none. */
export function worstTone(tones: ReadonlyArray<Tone>): Tone {
  return tones.reduce<Tone>((worst, tone) => (TONE_RANK[tone] > TONE_RANK[worst] ? tone : worst), "muted");
}

/** The Plan section: the plan's state, and BMAD's size when it ran. A plan waiting for approval is the user's. */
export function planFold(view: Pick<RunView, "plan" | "bmad" | "waitingFor">): FoldInfo {
  const { plan, bmad } = view;
  const parts: string[] = [];
  const tones: Tone[] = [];
  if (plan) {
    parts.push(plan.approved ? "approved" : plan.verdict ? `review: ${plan.verdict}` : "being written");
    tones.push(plan.approved ? "success" : "muted", planReviewTone(plan.openFindings), riskTone(plan.riskVerdict));
  }
  if (bmad && bmad.mode !== "off") parts.push(bmad.size ? `BMAD size ${bmad.size}` : "BMAD");
  if (view.waitingFor === "plan-approval") tones.push("you");
  return { summary: parts.join(" · ") || "no plan yet", tone: worstTone(tones) };
}

/** The PRs section: how many are done, and the worst thing any of them shows (a failing check, QA or CI is red). */
export function prsFold(view: Pick<RunView, "prs" | "waitingFor">): FoldInfo {
  const total = view.prs.length;
  const done = prsDone(view.prs);
  const tones = view.prs.flatMap((pr): Tone[] => [
    prStateTone(pr.state),
    checkTone(pr.check.result),
    qaPill(pr)?.tone ?? "muted",
    ciChip(pr)?.tone ?? "muted",
  ]);
  if (view.waitingFor === "ship-approval") tones.push("you");
  const tone = worstTone(tones);
  const trouble = tone === "danger" ? " · something failed" : tone === "warn" ? " · needs a look" : "";
  return { summary: `${done}/${total} done${trouble}`, tone };
}

/** The Cleanup section: what the app removed for merged PRs; a retry still pending is a warning. */
export function cleanupFold(records: ReadonlyArray<{ retry?: boolean }>): FoldInfo {
  const retry = records.filter((rec) => rec.retry).length;
  const summary = `${records.length} cleaned up${retry > 0 ? `, ${retry} to retry` : ""}`;
  return { summary, tone: retry > 0 ? "warn" : "muted" };
}

/** The Timeline section: how many events, nothing to act on. */
export function timelineFold(timeline: ReadonlyArray<unknown>): FoldInfo {
  return { summary: `${timeline.length} ${timeline.length === 1 ? "event" : "events"}`, tone: "muted" };
}

/** The sections whose data the section's own component reads: a fixed line. The tone comes from that component. */
export function sessionFold(tone: Tone = "muted"): FoldInfo {
  const summary = tone === "you" ? "Claude is waiting for you" : tone === "muted" ? "Cost and context of this tab" : "Needs a look";
  return { summary, tone };
}
export function agentsFold(): FoldInfo {
  return { summary: "Who ran when", tone: "muted" };
}
export function clickupFold(tone: Tone = "muted"): FoldInfo {
  return { summary: tone === "muted" ? "Ticket updates" : "Needs a look", tone };
}

/** The short word the collapsed rail shows when the run waits for the user: "Plan" or "Ship". */
export function waitingLabel(waitingFor: string | null | undefined): string | null {
  if (!waitingFor) return null;
  if (waitingFor === "plan-approval") return "Plan";
  if (waitingFor === "ship-approval") return "Ship";
  return waitingFor;
}
