import { describe, expect, it } from "vitest";
import { minutesText, relativeDate, runMatches } from "./words";

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
