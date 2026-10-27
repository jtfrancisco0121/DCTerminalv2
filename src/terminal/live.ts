/**
 * One PTY channel per terminal (tab) id, and the only place that opens a PTY
 * or writes to one (views never call `ptyOpen`/`ptyWrite` themselves). Bytes
 * are buffered until xterm attaches, so a late mount (or a StrictMode
 * remount) does not drop the first output or start a second process.
 *
 * Every open gets a token. A close (`dropLivePty`) or a newer open of the
 * same id makes it stale: a stale open starts nothing, flushes nothing and
 * reports no error. Keystrokes written while an open is pending are queued
 * and sent, in order, once it succeeds.
 */

import type { Channel } from "@tauri-apps/api/core";
import { createPtyChannel, ptyOpen, ptyWrite, type PtyPacket } from "../bridge";
import { reportError } from "../uiErrors";
import { createPtyBuffer, decodeBase64, type PtyBuffer } from "./buffer";

export type LivePty = {
  id: string;
  buffer: PtyBuffer;
  channel: Channel<PtyPacket>;
  /** When this process started. */
  startedAt: number;
  exitCode: number | null;
  /** The process exited (its code can be null after a signal). */
  exited: boolean;
};

const sessions = new Map<string, LivePty>();
/** Ids with an open in flight (before its `ptyOpen` settles). */
const opening = new Set<string>();
/** The current open's token per id; absent once the tab was closed. */
const openTokens = new Map<string, number>();
let lastToken = 0;
type QueuedWrite = { data: string; resolve: () => void; reject: (err: unknown) => void };
/** Keystrokes written while the id is opening. */
const queues = new Map<string, QueuedWrite[]>();
const listeners = new Set<() => void>();
const exitListeners = new Set<(id: string, code: number) => void>();

function notify(): void {
  for (const listener of listeners) listener();
}

/** Called whenever a PTY starts or exits. */
export function subscribeLivePty(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

/** Called once per process exit with the terminal id and exit code. */
export function subscribePtyExit(listener: (id: string, code: number) => void): () => void {
  exitListeners.add(listener);
  return () => exitListeners.delete(listener);
}

/** The exit code once the process exited (-1 after a signal), else null. */
export function ptyExitCode(id: string): number | null {
  const live = sessions.get(id);
  return live?.exited ? (live.exitCode ?? -1) : null;
}

/** A process started in this page runs under `id` (it has not exited). */
export function ptyRunning(id: string): boolean {
  const live = sessions.get(id);
  return !!live && !live.exited;
}

export function livePty(id: string): LivePty | undefined {
  return sessions.get(id);
}

export function isPtyOpening(id: string): boolean {
  return opening.has(id);
}

/** Fresh buffer and channel. Replaces any previous session for this id. */
export function beginLivePty(id: string): LivePty {
  const buffer = createPtyBuffer();
  const live: LivePty = {
    id,
    buffer,
    channel: createPtyChannel(() => {}),
    startedAt: Date.now(),
    exitCode: null,
    exited: false,
  };
  live.channel = createPtyChannel((packet) => {
    if (packet.kind === "exit") {
      live.exitCode = packet.code;
      live.exited = true;
      buffer.pushExit(packet.code);
      // A closed tab or a replaced (restarted) process is not news.
      if (sessions.get(live.id) !== live) return;
      for (const listener of exitListeners) listener(live.id, packet.code ?? -1);
      notify();
      return;
    }
    if (packet.data) buffer.pushData(decodeBase64(packet.data));
  });
  sessions.set(id, live);
  notify();
  return live;
}

/** How an open ended. `superseded`: a newer open of the id took over (or none was needed). */
export type OpenOutcome = "opened" | "superseded" | "closed" | "failed";

export type OpenPlan<T = void> = {
  /**
   * Runs first (a Claude tab asks whether its session can resume). If the
   * tab was closed or reopened meanwhile, the open stops here.
   */
  before?: () => Promise<T>;
  /** Size and mode of the new process, read after `before` (still current). */
  start: (before: T) => { cols: number; rows: number; resume: boolean };
  /** Only for the current open: the `ptyOpen` call failed. */
  onError?: (message: string) => void;
  /** First open of a page: do nothing when the id is opening or already has a session. */
  ifIdle?: boolean;
};

function dropQueue(id: string, send: boolean): void {
  const queued = queues.get(id) ?? [];
  queues.delete(id);
  for (const item of queued) {
    if (send) ptyWrite(id, item.data).then(item.resolve, item.reject);
    else item.resolve();
  }
}

/**
 * Opens the PTY of `id`: a fresh record replaces the old one (Restart,
 * Resume) and the id stays "opening" until `ptyOpen` settles. Resolves with
 * how it ended; never rejects.
 */
export async function openLivePty<T = void>(id: string, plan: OpenPlan<T>): Promise<OpenOutcome> {
  if (plan.ifIdle && (opening.has(id) || sessions.has(id))) return "superseded";
  const token = ++lastToken;
  openTokens.set(id, token);
  opening.add(id);
  const stale = (): OpenOutcome | null => {
    const current = openTokens.get(id);
    if (current === token) return null;
    return current === undefined ? "closed" : "superseded";
  };
  try {
    const value = plan.before ? await plan.before() : (undefined as T);
    const gone = stale();
    if (gone) return gone;
    const { cols, rows, resume } = plan.start(value);
    const live = beginLivePty(id);
    await ptyOpen({ id, cols, rows, resume, onOutput: live.channel });
    const lost = stale();
    if (lost) return lost;
    opening.delete(id);
    dropQueue(id, true);
    return "opened";
  } catch (err) {
    // A stale open leaves the id's state, and the error, to whoever made it stale.
    const lost = stale();
    if (lost) return lost;
    opening.delete(id);
    dropQueue(id, false);
    plan.onError?.(err instanceof Error ? err.message : String(err));
    return "failed";
  }
}

/**
 * Writes to the PTY of `id`. While an open is pending the data is queued and
 * sent after it succeeds (dropped, quietly, when it fails or is replaced).
 */
export function writeLivePty(id: string, data: string): Promise<void> {
  if (opening.has(id)) {
    return new Promise<void>((resolve, reject) => {
      const queue = queues.get(id) ?? [];
      queue.push({ data, resolve, reject });
      queues.set(id, queue);
    });
  }
  return ptyWrite(id, data);
}

/** How long a rejected keystroke waits for the PTY's exit event before it is shown. */
export const KEYSTROKE_SETTLE_MS = 300;

/**
 * Keystrokes to the PTY (a drop or paste reaches it the same way, through
 * xterm). A failed write is shown once per terminal, not per keystroke. An
 * exited or still-opening terminal refusing them is expected, not news.
 */
export function sendPtyInput(id: string, data: string): void {
  void writeLivePty(id, data).catch((err: unknown) => {
    // A PTY that just died rejects the write before its exit event lands, so
    // decide once that event has had time to arrive. A restart in between
    // replaces the session, and the old process's refusal is not news either.
    const rejectedBy = livePty(id);
    window.setTimeout(() => {
      const live = livePty(id);
      if (!live || live !== rejectedBy || live.exited || isPtyOpening(id)) return;
      reportError("Keystrokes not sent", err, { notify: true, once: `Keystrokes not sent:${id}` });
    }, KEYSTROKE_SETTLE_MS);
  });
}

/** The tab closed: its session, pending open and queued keystrokes go. */
export function dropLivePty(tabId: string): void {
  sessions.delete(tabId);
  opening.delete(tabId);
  openTokens.delete(tabId);
  dropQueue(tabId, false);
}
