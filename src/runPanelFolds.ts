/**
 * Which run panel sections a run has folded, kept in localStorage under its own key (`dcv2.runPanel.<windowId>` holds
 * the panel's width and collapsed state and never grows with runs). A JSON map of run id to the section ids folded and
 * the time of the last change; every write keeps only the newest 50 runs. Storage that throws or holds junk gives no folds.
 */
import { FOLD_TITLES, type FoldId } from "./run/words";

export const FOLDS_KEY = "dcv2.runPanelFolds";
/** How many runs keep their folds. */
export const MAX_FOLDED_RUNS = 50;

type Entry = { at: number; folded: FoldId[] };
type FoldMap = Record<string, Entry>;

const isFoldId = (value: unknown): value is FoldId => typeof value === "string" && Object.prototype.hasOwnProperty.call(FOLD_TITLES, value);

/** The stored map: entries that are not `{at: number, folded: FoldId[]}` are dropped; junk is an empty map. */
function readMap(): FoldMap {
  try {
    const raw = window.localStorage.getItem(FOLDS_KEY);
    if (!raw) return {};
    const parsed: unknown = JSON.parse(raw);
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) return {};
    const map: FoldMap = {};
    for (const [runId, entry] of Object.entries(parsed)) {
      if (!entry || typeof entry !== "object") continue;
      const { at, folded } = entry as Record<string, unknown>;
      if (typeof at !== "number" || !Number.isFinite(at) || !Array.isArray(folded)) continue;
      map[runId] = { at, folded: folded.filter(isFoldId) };
    }
    return map;
  } catch {
    return {};
  }
}

/** The sections `runId` has folded (none when unknown). */
export function loadFolds(runId: string): FoldId[] {
  return readMap()[runId]?.folded ?? [];
}

/** Remember `folded` for `runId` (the time is now) and drop all but the newest 50 runs. */
export function saveFolds(runId: string, folded: readonly FoldId[], now: number = Date.now()): void {
  try {
    const map = readMap();
    if (folded.length === 0) delete map[runId];
    else map[runId] = { at: now, folded: [...new Set(folded)] };
    const newest = Object.entries(map)
      .sort(([, a], [, b]) => b.at - a.at)
      .slice(0, MAX_FOLDED_RUNS);
    window.localStorage.setItem(FOLDS_KEY, JSON.stringify(Object.fromEntries(newest)));
  } catch {
    // Storage is full or blocked: the folds are just not remembered.
  }
}
