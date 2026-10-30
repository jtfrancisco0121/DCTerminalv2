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
  params: { folder: string; request: string; clickup?: { ticketIds: string[] } | null };
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
export const dispatchEnqueue = (params: StartParams, label: string) => invoke<string>("dispatch_enqueue", { params, label });

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
