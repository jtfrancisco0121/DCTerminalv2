/**
 * Backend push events (observe/notify.rs: `tab-state`, `run-state`, `stall`)
 * with a polling fallback. A view refreshes on mount, on an event and on
 * window focus, and polls in between: slowly while events arrive, at its old
 * interval when they do not (or the subscription failed), so it never freezes.
 */

import { useEffect, useRef } from "react";
import { listen } from "@tauri-apps/api/event";
import { reportError } from "../uiErrors";

export const TAB_STATE_EVENT = "tab-state";
export const RUN_STATE_EVENT = "run-state";
export const STALL_EVENT = "stall";

/** While events arrive, polling slows to this (at least) and falls back after this many silent periods. */
export const SLOW_MS = 15_000;
const SILENT_PERIODS = 3;

type Options = {
  /** The poll interval with no events: the view's old interval. */
  fastMs: number;
  /** The poll interval while events arrive (15 s or more). */
  slowMs?: number;
};

/**
 * `refresh` may return a promise: the next poll starts only after it settled.
 * `eventName` may be several names; any of them counts.
 */
export function usePushRefresh(eventName: string | readonly string[], refresh: () => unknown, options: Options): void {
  const refreshRef = useRef(refresh);
  refreshRef.current = refresh;
  const names = typeof eventName === "string" ? [eventName] : [...eventName];
  const key = names.join("|");
  const { fastMs } = options;
  const slowMs = Math.max(options.slowMs ?? SLOW_MS, SLOW_MS);

  useEffect(() => {
    let disposed = false;
    let timer: number | undefined;
    let lastEvent = Number.NEGATIVE_INFINITY;
    const unlistens: Array<() => void> = [];

    const schedule = () => {
      window.clearTimeout(timer);
      if (disposed) return;
      const delay = Date.now() - lastEvent < SILENT_PERIODS * slowMs ? slowMs : fastMs;
      timer = window.setTimeout(tick, delay);
    };
    const tick = () => {
      window.clearTimeout(timer);
      new Promise((resolve) => resolve(refreshRef.current()))
        .catch(() => undefined) // The view reports its own failures.
        .finally(schedule);
    };
    const onEvent = () => {
      lastEvent = Date.now();
      tick();
    };

    for (const name of key.split("|")) {
      listen(name, onEvent).then(
        (unlisten) => (disposed ? unlisten() : unlistens.push(unlisten)),
        (err: unknown) => reportError(`Live updates (${name})`, err, { once: `listen-${name}` }),
      );
    }
    window.addEventListener("focus", tick);
    tick();
    return () => {
      disposed = true;
      window.clearTimeout(timer);
      window.removeEventListener("focus", tick);
      for (const unlisten of unlistens) unlisten();
    };
  }, [key, fastMs, slowMs]);
}
