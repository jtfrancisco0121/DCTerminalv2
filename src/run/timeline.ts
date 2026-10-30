/** The fleet timeline: types of `run_timeline` and the layout of its lanes (pure, tested). */
import { invoke } from "@tauri-apps/api/core";

export type Lane = {
  id: string;
  label: string;
  description: string | null;
  state: string;
  startMs: number | null;
  finishMs: number | null;
  costUsd: number | null;
};
export type Band = { kind: "stage" | "wait"; name: string; startMs: number; endMs: number };
export type Timeline = {
  lanes: Lane[];
  bands: Band[];
  workMinutes: number;
  waitMinutes: number;
  costUsd: number | null;
  startedMs: number | null;
  endedMs: number | null;
};

export const runTimeline = (runDir: string) => invoke<Timeline>("run_timeline", { runDir });

export type Bar = { lane: Lane; left: number; width: number; open: boolean; name: string };
export type Layout = { bars: Bar[]; waits: { left: number; width: number; name: string }[]; spanMs: number };

const usd = (v: number | null) => (v === null ? "cost unknown" : `$${v.toFixed(2)}`);
const clock = (ms: number | null) => (ms === null ? "unknown time" : new Date(ms).toISOString().slice(11, 16) + " UTC");

/** What a screen reader says for a bar: label, the state as a word, the start and the cost. */
export function barName(lane: Lane): string {
  const state = lane.state === "unfinished" ? "unfinished (no finish recorded)" : lane.state;
  return `${lane.label}${lane.description ? `, ${lane.description}` : ""}: ${state}, started ${clock(lane.startMs)}, ${usd(lane.costUsd)}`;
}

/**
 * Lanes in order (the orchestrator first, then by start), each as a left/width in percent of the run's span.
 * A lane with no finish is drawn open to the end of the span; a lane with no start sits at the start.
 */
export function layoutTimeline(t: Timeline): Layout {
  const times = [t.startedMs, t.endedMs, ...t.lanes.flatMap((l) => [l.startMs, l.finishMs]), ...t.bands.flatMap((b) => [b.startMs, b.endMs])].filter(
    (v): v is number => v !== null,
  );
  const lo = times.length ? Math.min(...times) : 0;
  const hi = times.length ? Math.max(...times) : 1;
  const span = Math.max(hi - lo, 1);
  const pct = (ms: number) => ((ms - lo) / span) * 100;
  const order = [...t.lanes].sort((a, b) => {
    if (a.label === "orchestrator" || b.label === "orchestrator") return a.label === "orchestrator" ? -1 : 1;
    return (a.startMs ?? Number.MAX_SAFE_INTEGER) - (b.startMs ?? Number.MAX_SAFE_INTEGER);
  });
  const bars = order.map((lane) => {
    const start = lane.startMs ?? lo;
    const open = lane.finishMs === null;
    const end = lane.finishMs ?? hi;
    return { lane, left: pct(start), width: Math.max(pct(end) - pct(start), 0.8), open, name: barName(lane) };
  });
  const waits = t.bands
    .filter((b) => b.kind === "wait")
    .map((b) => ({ left: pct(b.startMs), width: Math.max(pct(b.endMs) - pct(b.startMs), 0.8), name: b.name }));
  return { bars, waits, spanMs: span };
}

export const minutesText = (m: number) => (m < 1 ? "under a minute" : m >= 60 ? `${Math.floor(m / 60)} h ${Math.round(m % 60)} min` : `${Math.round(m)} min`);
export const costText = (v: number | null) => (v === null ? "cost unknown" : `$${v.toFixed(2)}`);
