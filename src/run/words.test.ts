import { afterEach, describe, expect, it, vi } from "vitest";
import { localTime, minutesText, relativeDate, runMatches } from "./words";

describe("runs list words", () => {
  const run = { title: "Add multiply()", folder: "/w/App", runId: "r-20261027-0900-abcd", ticketIds: ["86b1aa001", "QA-129"] };

  it("matches the title, folder, run id and every ticket id, ignoring case", () => {
    for (const query of ["multiply", "/w/app", "abcd", "86B1AA001", "qa-129", "  "]) {
      expect(runMatches(run, query), query).toBe(true);
    }
    expect(runMatches(run, "QA-7")).toBe(false);
    expect(runMatches({ ...run, title: null, folder: null, ticketIds: undefined }, "zzz")).toBe(false);
  });

  it("words minutes", () => {
    expect(minutesText(0)).toBe("0 min");
    expect(minutesText(34.6)).toBe("35 min");
    expect(minutesText(125)).toBe("2 h 5 min");
    expect(minutesText(null)).toBe("—");
    expect(minutesText(undefined)).toBe("—");
  });

  it("words dates relative to now (local days)", () => {
    const now = new Date(2026, 9, 27, 12, 0);
    const at = (day: number, hour: number) => new Date(2026, 9, day, hour, 0).toISOString();
    expect(relativeDate(at(27, 1), now)).toBe("today");
    expect(relativeDate(at(26, 23), now)).toBe("yesterday");
    expect(relativeDate(at(24, 12), now)).toBe("3 days ago");
    expect(relativeDate(at(1, 12), now)).toBe(at(1, 12).slice(0, 10));
    expect(relativeDate(null, now)).toBe("");
    expect(relativeDate("not a date", now)).toBe("");
  });
});

describe("localTime", () => {
  afterEach(() => {
    vi.unstubAllEnvs();
  });

  it("shows the system zone's HH:MM with the UTC time in the title", () => {
    vi.stubEnv("TZ", "Asia/Manila");
    const manila = localTime("2026-10-27T14:04:27Z");
    expect(manila?.short).toBe("22:04");
    expect(manila?.title).toContain("(UTC 2026-10-27T14:04:27Z)");
    vi.stubEnv("TZ", "UTC");
    expect(localTime("2026-10-27T14:04:27Z")?.short).toBe("14:04");
  });

  it("never shows 24:xx at midnight, and takes epoch milliseconds", () => {
    vi.stubEnv("TZ", "UTC");
    expect(localTime("2026-10-28T00:05:00Z")?.short).toBe("00:05");
    expect(localTime(Date.parse("2026-10-28T00:05:00Z"))).toEqual(localTime("2026-10-28T00:05:00Z"));
  });

  it("returns null for garbage", () => {
    expect(localTime("not a time")).toBeNull();
  });
});
