/** The dispatcher's commands (src-tauri/src/run/dispatch.rs): the queue, the slot holders and the cap. */
import { invoke } from "@tauri-apps/api/core";

export type QueueEntry = {
  id: string;
  label: string;
  /** `queued` or `starting`. */
  state: string;
  reason: string | null;
  accountId: string;
  queuedAt: string;
  batchId: string | null;
  params: {
    folder: string;
    request: string;
    clickup?: { ticketIds: string[] } | null;
    /** One run of an eval group; its usage hold is never skipped, not even by Start now. */
    eval?: { group: string; index: number; of: number; source: string } | null;
  };
};
export type Holder = { runDir: string; runId: string; stage: string | null; why: string };
export type DispatchOverview = { cap: number; usageHoldPercent: number | null; queue: QueueEntry[]; holders: Holder[]; atCap: boolean };

export const dispatchOverview = () => invoke<DispatchOverview>("dispatch_overview");
export const dispatchMove = (id: string, up: boolean) => invoke<boolean>("dispatch_move", { id, up });
export const dispatchCancel = (id: string) => invoke<void>("dispatch_cancel", { id });
export const dispatchStartNow = (id: string) => invoke<void>("dispatch_start_now", { id });
export const dispatchRelease = (runDir: string) => invoke<void>("dispatch_release", { runDir });
/** The pieces of a start the dispatcher stores (the same fields `start_run` takes). */
export type StartParams = {
  windowId?: string | null;
  folder: string;
  request: string;
  role: string;
  clickup?: unknown;
  reviewMode?: string | null;
  bmadMode?: string | null;
  repos?: string[] | null;
  maxImplementers?: number | null;
  runMode?: string | null;
};
/** One run per ticket group: a new batch of queue entries (the dispatcher starts them as slots allow). */
export const dispatchRuns = (base: StartParams, groups: { clickup: { refs: unknown[]; ticketIds: string[] }; label: string }[]) =>
  invoke<string>("dispatch_runs", { base, groups });
/** "Evaluate N times": a new eval batch of N queue entries from a finished run (refused with the reason). */
export const dispatchEvalStart = (windowId: string | null, sourceRunDir: string, n: number) =>
  invoke<string>("dispatch_eval_start", { windowId, sourceRunDir, n });
export const dispatchEnqueue = (params: StartParams, label: string) => invoke<string>("dispatch_enqueue", { params, label });

export type BatchRow = {
  entryId: string;
  label: string;
  ticketIds: string[];
  removed: boolean;
  runDir: string | null;
  /** `queued`, `starting` or the run's stage. */
  stage: string;
  waitingFor: string | null;
  prUrl: string | null;
  ci: string | null;
};
/** An eval group in the overview: k of n runs have ended. */
export type EvalBatch = { n: number; source: string; finished: number };
export type Batch = { id: string; createdAt: string; rows: BatchRow[]; waitingForYou: number; eval?: EvalBatch | null };
export const batchOverview = () => invoke<Batch[]>("batch_overview");
export const batchRemoveRun = (batchId: string, entryId: string) => invoke<void>("batch_remove_run", { batchId, entryId });

export type WaveCard = { n: number; title: string; ticketIds: string[]; state: "done" | "building" | "ready" | "waiting"; reason: string | null };
export type Epic = { runDir: string; runId: string; problem: string | null; cards: WaveCard[] };
export const epicsOverview = () => invoke<Epic[]>("epics_overview");
export const epicStartWave = (runDir: string, n: number) => invoke<string>("epic_start_wave", { runDir, n });

/** One line for a queue entry's state: its reason, or that it starts when a slot is free. */
export function queueStateText(e: QueueEntry): string {
  if (e.state === "starting") return "starting now";
  return e.reason ?? "starts when a slot is free";
}

/** How many of `count` starts would queue given the slots in use: the Start label says so. */
export function queueCount(o: Pick<DispatchOverview, "cap" | "holders" | "queue">, count: number): number {
  const free = Math.max(0, o.cap - o.holders.length - o.queue.length);
  return Math.max(0, count - free);
}

/** What the Eval setup dialog shows before it starts anything (dispatch.rs `EvalPreview`). */
export type EvalPreview = {
  title: string;
  /** `run.usageHoldPercent`; null when no hold is set. */
  holdPercent: number | null;
  /** The latest five-hour reading of the window's account; null when there is none. */
  fiveHour: { percent: number; resetsAt: number | null } | null;
  /** Repos the runs build on the source run's base commits. */
  basedRepos: string[];
  /** Repos the source has no base for: they use the default branch. */
  defaultBranchRepos: string[];
};
export const dispatchEvalPreview = (windowId: string | null, sourceRunDir: string) =>
  invoke<EvalPreview>("dispatch_eval_preview", { windowId, sourceRunDir });
