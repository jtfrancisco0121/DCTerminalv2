/**
 * Display helpers for Claude 5-hour and 7-day limits. Claude terminal tabs
 * report utilization and a reset time through the status line DCTerminalv2
 * sets. Usage that is missing or stale reads as unknown, never 0%.
 */

export type RateWindow = {
  rateLimitType: string;
  label: string;
  utilization: number | null;
  resetsAt: number | null;
  status: string;
  seenAtMs: number;
};

/** One saved reading of a window. */
export type Reading = { atMs: number; utilization: number | null; resetsAt: number | null };

export type WindowHistory = { rateLimitType: string; readings: Reading[] };

/** The 5-hour window from this percent warns on the start screen (it never blocks starting). */
export const NEAR_LIMIT_PERCENT = 80;

export type UsageTone = "ok" | "warn" | "hot" | "muted";

export type LimitLine = {
  text: string;
  tone: UsageTone;
  title: string;
};

/** Where usage readings come from (status bar title). */
export const USAGE_NOTE =
  "Claude terminal tabs report it through the status line DCTerminalv2 sets, unless you set your own.";

/** How long each window lasts: a reading with no reset time is stale after it. */
const WINDOW_MS: Record<string, number> = {
  five_hour: 5 * 3_600_000,
  seven_day: 7 * 24 * 3_600_000,
};

export function limitTone(window: RateWindow): UsageTone {
  if (window.status === "rejected") return "hot";
  if (window.status === "allowed_warning") return "warn";
  if (window.utilization != null && window.utilization >= 80) return "warn";
  return "ok";
}

export function formatReset(resetsAt: number | null): string | null {
  if (resetsAt == null) return null;
  const date = new Date(resetsAt * 1000);
  if (Number.isNaN(date.getTime())) return null;
  return date.toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" });
}

export function formatWindowLine(window: RateWindow): string {
  const head =
    window.utilization == null
      ? `Claude ${window.label}`
      : `Claude ${window.label} ${Math.round(window.utilization)}%`;
  const reset = formatReset(window.resetsAt);
  return reset ? `${head} · resets ${reset}` : head;
}

/**
 * The window has reset since this reading (for example a reading saved before
 * a restart), so its percent no longer applies. With no reset time, a reading
 * older than its window's length is stale too.
 */
export function isStale(window: RateWindow, now = Date.now()): boolean {
  if (window.resetsAt != null) return window.resetsAt * 1000 <= now;
  const length = WINDOW_MS[window.rateLimitType];
  return length != null && now - window.seenAtMs > length;
}

/**
 * The highest current window, in percent: null when unknown (no reading, no
 * percent, or every reading stale). Never 0 for unknown.
 */
export function knownUsagePercent(windows: RateWindow[], now = Date.now()): number | null {
  const values = windows
    .filter((window) => !isStale(window, now))
    .map((window) => window.utilization)
    .filter((value): value is number => typeof value === "number" && Number.isFinite(value));
  return values.length ? Math.round(Math.max(...values)) : null;
}

/** Status-bar line. Prefers the current 5-hour window, then any current one. Unknown until the first update or when every reading is stale. */
export function statusLimit(windows: RateWindow[], now = Date.now(), history: WindowHistory[] = []): LimitLine {
  if (windows.length === 0) {
    return {
      text: "Claude usage unknown",
      tone: "muted",
      title: USAGE_NOTE,
    };
  }
  const preferred = (list: RateWindow[]) =>
    list.find((window) => window.rateLimitType === "five_hour") ?? list[0];
  // A stale 5h reading falls back to a window still current (as the usage
  // percent does); unknown only when every reading is stale.
  const current = preferred(windows.filter((window) => !isStale(window, now)));
  const primary = current ?? preferred(windows);
  const lines = windows.map((window) => {
    if (isStale(window, now)) return `Claude ${window.label} has reset since the last reading`;
    const trend = trendText(window, history);
    return trend ? `${formatWindowLine(window)} · ${trend}` : formatWindowLine(window);
  });
  const title = [USAGE_NOTE, ...lines, `Last reported ${formatSeen(primary.seenAtMs)}`].join("\n");
  if (!current) {
    return { text: `Claude usage unknown · ${primary.label} reset since the last reading`, tone: "muted", title };
  }
  return { text: formatWindowLine(current), tone: limitTone(current), title };
}

/** The saved readings of the window's current reset period: same window type, same reset time, with a percent. */
export function periodReadings(window: RateWindow, history: WindowHistory[]): Reading[] {
  return (history.find((h) => h.rateLimitType === window.rateLimitType)?.readings ?? []).filter(
    (r) => r.utilization != null && r.resetsAt === window.resetsAt,
  );
}

/** The percent a meter fills to; null (hatched, never 0) when the reading is stale or has no percent. */
export function meterValue(window: RateWindow, now = Date.now()): number | null {
  if (isStale(window, now) || window.utilization == null || !Number.isFinite(window.utilization)) return null;
  return window.utilization;
}

/**
 * "rising: +12 points in 40 min" from the saved readings of the window's
 * current reset period (the same reset time); null with fewer than two
 * readings that carry a percent.
 */
export function trendText(window: RateWindow, history: WindowHistory[]): string | null {
  const readings = periodReadings(window, history);
  if (readings.length < 2) return null;
  const first = readings[0];
  const last = readings[readings.length - 1];
  const delta = Math.round((last.utilization as number) - (first.utilization as number));
  const minutes = Math.max(1, Math.round((last.atMs - first.atMs) / 60_000));
  const span = minutes >= 120 ? `${Math.round(minutes / 60)} h` : `${minutes} min`;
  if (delta === 0) return `steady over ${span}`;
  return `${delta > 0 ? "rising" : "falling"}: ${delta > 0 ? "+" : ""}${delta} points in ${span}`;
}

/** A warning for the start screen when the current 5-hour window is at 80% or more; null otherwise. */
export function nearLimitWarning(windows: RateWindow[] | null | undefined, now = Date.now()): string | null {
  const five = (windows ?? []).find((window) => window.rateLimitType === "five_hour");
  if (!five || isStale(five, now) || five.utilization == null || five.utilization < NEAR_LIMIT_PERCENT) return null;
  const reset = formatReset(five.resetsAt);
  return `Claude's 5-hour limit is at ${Math.round(five.utilization)}%${reset ? ` (resets ${reset})` : ""}. A big run may stop early; you can still start it.`;
}

export function formatSeen(seenAtMs: number): string {
  const date = new Date(seenAtMs);
  if (Number.isNaN(date.getTime())) return "";
  return date.toLocaleString(undefined, {
    hour: "numeric",
    minute: "2-digit",
    month: "short",
    day: "numeric",
  });
}
