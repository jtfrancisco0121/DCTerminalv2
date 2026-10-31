import { describe, expect, it } from "vitest";
import {
  formatWindowLine,
  isStale,
  knownUsagePercent,
  limitTone,
  meterValue,
  nearLimitWarning,
  NEAR_LIMIT_PERCENT,
  periodReadings,
  statusLimit,
  trendText,
  type RateWindow,
} from "./limits";

function window(overrides: Partial<RateWindow> = {}): RateWindow {
  return {
    rateLimitType: "five_hour",
    label: "5h",
    utilization: 42,
    resetsAt: Date.UTC(2099, 9, 9, 15, 10) / 1000,
    status: "allowed",
    seenAtMs: 1,
    ...overrides,
  };
}

describe("Claude limit display", () => {
  it("says unknown before the first update, never 0%", () => {
    const line = statusLimit([]);
    expect(line.text).toBe("Claude usage unknown");
    expect(line.text).not.toMatch(/0%/);
    expect(line.tone).toBe("muted");
    expect(line.title).toMatch(/terminal tabs report it through the status line/i);
  });

  it("formats the 5-hour window and does not invent a percent", () => {
    const dated = statusLimit([window({ utilization: null })]);
    expect(dated.text.startsWith("Claude 5h")).toBe(true);
    expect(dated.text).not.toMatch(/%/);
    expect(dated.text).toContain("resets");
    const filled = statusLimit([
      window({ utilization: 42.4 }),
      window({ rateLimitType: "seven_day", label: "7d", utilization: 10 }),
    ]);
    expect(filled.text.startsWith("Claude 5h 42%")).toBe(true);
    expect(filled.title).toContain("Claude 7d 10%");
  });

  it("turns yellow at 80% or allowed_warning and red when rejected", () => {
    expect(limitTone(window({ utilization: 80 }))).toBe("warn");
    expect(limitTone(window({ utilization: 10, status: "allowed_warning" }))).toBe("warn");
    expect(limitTone(window({ status: "rejected", utilization: 10 }))).toBe("hot");
    expect(limitTone(window({ utilization: 42 }))).toBe("ok");
  });

  it("marks a reading from before the window reset as old", () => {
    const now = Date.UTC(2026, 9, 10, 12, 0);
    const old = window({ resetsAt: Date.UTC(2026, 9, 10, 9, 0) / 1000, utilization: 91 });
    expect(isStale(old, now)).toBe(true);
    const line = statusLimit([old], now);
    expect(line.text).toBe("Claude usage unknown · 5h reset since the last reading");
    expect(line.tone).toBe("muted");
    expect(line.title).toContain("Last reported");
    expect(isStale(window({ resetsAt: null, seenAtMs: now - 60_000 }), now)).toBe(false);
  });

  it("shows the current 7d window when only the 5h reading is stale, matching the usage percent", () => {
    const now = Date.UTC(2026, 9, 10, 12, 0);
    const windows = [
      window({ resetsAt: Date.UTC(2026, 9, 10, 9, 0) / 1000, utilization: 91 }),
      window({ rateLimitType: "seven_day", label: "7d", utilization: 40, resetsAt: Date.UTC(2026, 9, 14) / 1000 }),
    ];
    const line = statusLimit(windows, now);
    expect(line.text).not.toMatch(/unknown/);
    expect(line.text.startsWith("Claude 7d 40%")).toBe(true);
    expect(knownUsagePercent(windows, now)).toBe(40);
    expect(line.title).toContain("Claude 5h has reset since the last reading");
  });

  it("a stale 5h falls back to the current 7d; all stale is unknown", () => {
    const now = Date.UTC(2026, 9, 10, 10, 0);
    const seen = now - 60_000;
    const staleFive = window({
      utilization: 97,
      resetsAt: Date.UTC(2026, 9, 10, 9, 0) / 1000,
      status: "allowed_warning",
      seenAtMs: seen,
    });
    const sevenReset = Date.UTC(2026, 9, 14, 8, 0) / 1000;
    const currentSeven = window({ rateLimitType: "seven_day", label: "7d", utilization: 30, resetsAt: sevenReset, seenAtMs: seen });
    const line = statusLimit([staleFive, currentSeven], now);
    expect(line.text).toBe(formatWindowLine(currentSeven));
    expect(line.text.startsWith("Claude 7d 30% · resets ")).toBe(true);
    expect(line.tone).toBe("ok");
    expect(line.title).toContain("Claude 5h has reset since the last reading");

    const staleSeven = { ...currentSeven, resetsAt: Date.UTC(2026, 9, 9, 8, 0) / 1000 };
    const unknown = statusLimit([staleFive, staleSeven], now);
    expect(unknown.text).toBe("Claude usage unknown · 5h reset since the last reading");
    expect(unknown.tone).toBe("muted");
    expect(unknown.title).toContain("Claude 5h has reset since the last reading");
  });

  it("a reading with no reset time is stale once its window's length has passed", () => {
    const now = Date.UTC(2026, 9, 10, 12, 0);
    const hour = 3_600_000;
    expect(isStale(window({ resetsAt: null, seenAtMs: now - 6 * hour }), now)).toBe(true);
    expect(isStale(window({ resetsAt: null, seenAtMs: now - 4 * hour }), now)).toBe(false);
    const week = window({ rateLimitType: "seven_day", label: "7d", resetsAt: null });
    expect(isStale({ ...week, seenAtMs: now - 6 * hour }, now)).toBe(false);
    expect(isStale({ ...week, seenAtMs: now - 8 * 24 * hour }, now)).toBe(true);
  });

  it("usage is the highest window still current; stale or missing is unknown, not 0", () => {
    const now = Date.UTC(2026, 9, 10, 12, 0);
    const later = Date.UTC(2026, 9, 10, 15, 0) / 1000;
    const earlier = Date.UTC(2026, 9, 10, 9, 0) / 1000;
    expect(knownUsagePercent([], now)).toBeNull();
    expect(knownUsagePercent([window({ utilization: null, resetsAt: later })], now)).toBeNull();
    // Reset since the reading: its 95% no longer applies.
    expect(knownUsagePercent([window({ utilization: 95, resetsAt: earlier })], now)).toBeNull();
    expect(
      knownUsagePercent(
        [
          window({ utilization: 95, resetsAt: earlier }),
          window({ rateLimitType: "seven_day", label: "7d", utilization: 40.4, resetsAt: later }),
        ],
        now,
      ),
    ).toBe(40);
    expect(knownUsagePercent([window({ utilization: 0, resetsAt: later })], now)).toBe(0);
  });
});

describe("rate-limit history and the near-limit warning", () => {
  const reading = (minutes: number, utilization: number | null, resetsAt = window().resetsAt) => ({
    atMs: minutes * 60_000,
    utilization,
    resetsAt,
  });
  const history = (readings: ReturnType<typeof reading>[]) => [{ rateLimitType: "five_hour", readings }];

  it("words the trend of the current reset period and says nothing with fewer than two readings", () => {
    expect(trendText(window(), history([reading(0, 30), reading(40, 42)]))).toBe("rising: +12 points in 40 min");
    expect(trendText(window(), history([reading(0, 50), reading(180, 38)]))).toBe("falling: -12 points in 3 h");
    expect(trendText(window(), history([reading(0, 42), reading(10, 42)]))).toBe("steady over 10 min");
    expect(trendText(window(), history([reading(0, 30)]))).toBeNull();
    expect(trendText(window(), history([reading(0, null), reading(5, 40)]))).toBeNull();
    expect(trendText(window(), [])).toBeNull();
    // Readings of the previous window (another reset time) do not count.
    expect(trendText(window(), history([reading(0, 90, 1), reading(5, 10)]))).toBeNull();
  });

  it("keeps the readings of the current reset period only, and a meter is unknown when stale or without a percent", () => {
    const kept = periodReadings(window(), history([reading(0, 30), reading(5, null), reading(9, 90, 1), reading(10, 40)]));
    expect(kept.map((r) => r.utilization)).toEqual([30, 40]);
    expect(periodReadings(window({ rateLimitType: "seven_day" }), history([reading(0, 30)]))).toEqual([]);
    expect(meterValue(window({ utilization: 42 }))).toBe(42);
    expect(meterValue(window({ utilization: 0 }))).toBe(0);
    expect(meterValue(window({ utilization: null }))).toBeNull();
    expect(meterValue(window({ resetsAt: 1 }))).toBeNull();
  });

  it("puts the trend and the reset time in the status bar tooltip", () => {
    const line = statusLimit([window()], Date.now(), history([reading(0, 30), reading(40, 42)]));
    expect(line.title).toContain("rising: +12 points in 40 min");
    expect(line.title).toContain("resets");
    expect(statusLimit([window()]).title).not.toContain("rising");
  });

  it("warns at 80% on the 5-hour window and not below, stale, unknown or on the 7-day window", () => {
    expect(NEAR_LIMIT_PERCENT).toBe(80);
    expect(nearLimitWarning([window({ utilization: 80 })])).toContain("5-hour limit is at 80%");
    expect(nearLimitWarning([window({ utilization: 79.9 })])).toBeNull();
    expect(nearLimitWarning([window({ utilization: null })])).toBeNull();
    expect(nearLimitWarning([window({ utilization: 95, resetsAt: 1 })])).toBeNull();
    expect(nearLimitWarning([window({ rateLimitType: "seven_day", label: "7d", utilization: 95 })])).toBeNull();
    expect(nearLimitWarning(null)).toBeNull();
    expect(nearLimitWarning([window({ utilization: 91 })])).toContain("you can still start it");
  });
});
