/**
 * One PTY channel per terminal (tab) id. Bytes are buffered until xterm
 * attaches, so a late mount (or a StrictMode remount) does not drop the
 * first output or start a second process.
 */

import type { Channel } from "@tauri-apps/api/core";
import { createPtyChannel, type PtyPacket } from "../bridge";
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
const opening = new Set<string>();
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

export function markPtyOpening(id: string): boolean {
  if (opening.has(id) || sessions.has(id)) return false;
  opening.add(id);
  return true;
}

export function clearPtyOpening(id: string): void {
  opening.delete(id);
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
  opening.delete(id);
  notify();
  return live;
}

/** Restart or Resume on the same tab: a fresh record that replaces the old one. */
export function restartLivePty(id: string): LivePty {
  return beginLivePty(id);
}

export function dropLivePty(tabId: string): void {
  sessions.delete(tabId);
  opening.delete(tabId);
}
