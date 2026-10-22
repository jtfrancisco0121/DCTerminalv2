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
export function statusLimit(windows: RateWindow[], now = Date.now()): LimitLine {
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
  const lines = windows.map((window) =>
    isStale(window, now) ? `Claude ${window.label} has reset since the last reading` : formatWindowLine(window),
  );
  const title = [USAGE_NOTE, ...lines, `Last reported ${formatSeen(primary.seenAtMs)}`].join("\n");
  if (!current) {
    return { text: `Claude usage unknown · ${primary.label} reset since the last reading`, tone: "muted", title };
  }
  return { text: formatWindowLine(current), tone: limitTone(current), title };
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
