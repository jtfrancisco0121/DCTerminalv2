/**
 * "While you were away": what the run's timeline gained since the person last
 * looked at it. The marker (when the run panel was last seen) is kept per
 * window and run in localStorage; storage that fails shows nothing.
 */

import { useCallback, useEffect, useRef, useState } from "react";
import type { LogLine } from "./api";

/** The panel was out of view at least this long before the notice shows. */
export const AWAY_GAP_MS = 10 * 60_000;
/** At most this many rows are listed; the rest are counted. */
export const AWAY_ROWS = 10;

const markerKey = (windowId: string, runDir: string) => `dcv2.lastViewed.${windowId}.${runDir}`;

/** When the panel was last seen (epoch ms); null when never, or when storage fails or holds junk. */
export function readMarker(windowId: string, runDir: string): number | null {
  try {
    const raw = window.localStorage.getItem(markerKey(windowId, runDir));
    const ms = raw === null ? Number.NaN : Number(raw);
    return Number.isFinite(ms) ? ms : null;
  } catch {
    return null;
  }
}

export function writeMarker(windowId: string, runDir: string, ms: number): void {
  try {
    window.localStorage.setItem(markerKey(windowId, runDir), String(ms));
  } catch {
    // Storage is full or blocked: nothing is remembered, so nothing is shown next time.
  }
}

/** Timeline lines newer than `since`, but only after a gap of at least [`AWAY_GAP_MS`]; else none. */
export function linesWhileAway(timeline: readonly LogLine[], since: number, now: number): LogLine[] {
  if (now - since < AWAY_GAP_MS) return [];
  return timeline.filter((line) => {
    const at = line.time ? Date.parse(line.time) : Number.NaN;
    return Number.isFinite(at) && at > since;
  });
}

/**
 * The notice for the run panel: set when the panel appears (the run tab was
 * selected) or the window gets focus again after a gap with new timeline
 * lines; the marker follows the timeline while the panel is in view and no
 * notice is open. `ready` is true once the run was read.
 */
export function useAwayNotice(
  windowId: string,
  runDir: string,
  timeline: readonly LogLine[] | undefined,
  ready: boolean,
): { lines: LogLine[]; dismiss: () => void } | null {
  const [lines, setLines] = useState<LogLine[] | null>(null);
  const open = useRef<LogLine[] | null>(null);
  const focused = useRef(true);
  const timelineRef = useRef(timeline);
  timelineRef.current = timeline;

  const decide = useCallback(() => {
    if (open.current) return;
    const since = readMarker(windowId, runDir);
    const found = since === null ? [] : linesWhileAway(timelineRef.current ?? [], since, Date.now());
    if (found.length > 0) {
      open.current = found;
      setLines(found);
    }
  }, [windowId, runDir]);

  // The run tab was selected (the panel read its run): look once.
  const decided = useRef(false);
  useEffect(() => {
    if (!ready || decided.current) return;
    decided.current = true;
    decide();
  }, [ready, decide]);

  // The marker follows the timeline while the panel is in view and nothing is waiting to be dismissed.
  useEffect(() => {
    if (ready && focused.current && !open.current) writeMarker(windowId, runDir, Date.now());
  }, [ready, timeline, lines, windowId, runDir]);

  useEffect(() => {
    const onFocus = () => {
      focused.current = true;
      decide();
    };
    const onBlur = () => {
      // Looked at for the last time now.
      if (!open.current) writeMarker(windowId, runDir, Date.now());
      focused.current = false;
    };
    window.addEventListener("focus", onFocus);
    window.addEventListener("blur", onBlur);
    return () => {
      window.removeEventListener("focus", onFocus);
      window.removeEventListener("blur", onBlur);
      // Leaving the tab: the last look is now (a notice still open keeps the older marker).
      if (decided.current && !open.current) writeMarker(windowId, runDir, Date.now());
    };
  }, [windowId, runDir, decide]);

  const dismiss = useCallback(() => {
    open.current = null;
    setLines(null);
    writeMarker(windowId, runDir, Date.now());
  }, [windowId, runDir]);

  return lines ? { lines, dismiss } : null;
}
