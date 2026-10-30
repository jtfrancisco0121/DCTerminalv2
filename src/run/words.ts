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
  { key: "ci-failed", label: "A PR's CI failed on GitHub" },
  { key: "autopost-failed", label: "A ClickUp auto-post failed" },
  { key: "merged-cleaned", label: "A merged PR was cleaned up" },
];

/** The events the phone gets (src-tauri/src/observe/push.rs PUSH_KINDS). */
export const PUSH_KINDS: ReadonlyArray<{ key: string; label: string }> = ATTENTION_KINDS.filter((kind) =>
  ["stall", "check-failed", "ci-failed", "merged-cleaned", "autopost-failed"].includes(kind.key),
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

/**
 * A PR counts as done at these states: the same ones the backend counts for Active runs
 * (`pr_state_is_done` in src-tauri/src/observe/attention.rs), so the two never disagree.
 * A PR merged on GitHub stays `opened` in status.json.
 */
const DONE_STATES = ["reviewed", "drafted", "opened", "pushed", "local-only"];

export type EpicGroup = { key: string; id: string | null; title: string | null; prs: PrView[]; done: number };

/**
 * The PRs grouped by epic: plan order of first appearance, "No epic" last.
 * Fewer than two groups (an old run, one epic) gives [], and the caller shows a flat list.
 */
export function epicGroups(prs: PrView[]): EpicGroup[] {
  const groups: EpicGroup[] = [];
  const noEpic: EpicGroup = { key: "none", id: null, title: null, prs: [], done: 0 };
  for (const pr of prs) {
    const epic = pr.epic && typeof pr.epic.id === "string" && pr.epic.id ? pr.epic : null;
    let group = epic ? groups.find((g) => g.id === epic.id) : noEpic;
    if (!group) {
      group = { key: epic!.id, id: epic!.id, title: epic!.title || null, prs: [], done: 0 };
      groups.push(group);
    }
    group.prs.push(pr);
    if (pr.state && DONE_STATES.includes(pr.state)) group.done += 1;
  }
  if (noEpic.prs.length > 0) groups.push(noEpic);
  return groups.length >= 2 ? groups : [];
}

/** An epic's name: its title, else its id (e.g. in a column already headed "Epic"). */
export function epicName(epic: { id: string | null; title: string | null }): string {
  return epic.title || epic.id || "";
}

/** The full epic for a tooltip: `<title> (<id>)`, or the id alone. */
export function epicTooltip(epic: { id: string | null; title: string | null }): string | undefined {
  if (!epic.id) return undefined;
  return epic.title ? `${epic.title} (${epic.id})` : epic.id;
}

/** `Epic <title>`, `Epic <id>` when there is no title, or `No epic`. */
export function epicLabel(group: { id: string | null; title: string | null }): string {
  if (!group.id) return "No epic";
  return `Epic ${epicName(group)}`;
}

/** `3 PRs · 1 done` (the heading's counts). */
export function epicCounts(total: number, done: number): string {
  return `${total} ${total === 1 ? "PR" : "PRs"} · ${done} done`;
}

function idNumber(id: string): number {
  const m = /(\d+)$/.exec(id);
  return m ? Number(m[1]) : Number.MAX_SAFE_INTEGER;
}

/** Ids in stack order: a parent first, then its children, otherwise by PR number. */
export function stackOrder(items: { id: string; stackedOn?: string | null }[]): string[] {
  const ids = new Set(items.map((i) => i.id));
  const byNumber = (a: string, b: string) => idNumber(a) - idNumber(b) || a.localeCompare(b);
  const children = new Map<string, string[]>();
  const roots: string[] = [];
  for (const item of items) {
    const parent = item.stackedOn;
    if (parent && parent !== item.id && ids.has(parent)) children.set(parent, [...(children.get(parent) ?? []), item.id]);
    else roots.push(item.id);
  }
  const out: string[] = [];
  const seen = new Set<string>();
  const walk = (id: string) => {
    if (seen.has(id)) return;
    seen.add(id);
    out.push(id);
    for (const child of (children.get(id) ?? []).sort(byNumber)) walk(child);
  };
  for (const root of roots.sort(byNumber)) walk(root);
  // A stack loop in a damaged file: still list every id once.
  for (const item of items) walk(item.id);
  return out;
}

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
}): { text: string; tone: "success" | "danger" | "muted"; title: string } | null {
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
