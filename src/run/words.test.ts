import { afterEach, describe, expect, it, vi } from "vitest";
import pushRs from "../../src-tauri/src/observe/push.rs?raw";
import { ATTENTION_KINDS, ciChip, localTime, minutesText, PUSH_KINDS, relativeDate, runMatches, sizeBadgeText, sizeTitle } from "./words";

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

describe("attention kinds", () => {
  it("lists ci-failed in the Settings toggles and the phone push list, matching push.rs", () => {
    expect(ATTENTION_KINDS.map((k) => k.key)).toContain("ci-failed");
    expect(PUSH_KINDS.map((k) => k.key)).toContain("ci-failed");
    const body = /PUSH_KINDS: \[&str; \d+\] = \[([^\]]*)\]/.exec(pushRs)?.[1] ?? "";
    const rust = [...body.matchAll(/"([a-z-]+)"/g)].map((m) => m[1]).sort();
    expect(PUSH_KINDS.map((k) => k.key).sort()).toEqual(rust);
  });
});

describe("ciChip", () => {
  const pr = { prUrl: "https://github.com/o/r/pull/1", ciAt: "2026-10-27T14:04:27Z" };
  it("words each state with its tone, and hides without a URL or a reading", () => {
    vi.stubEnv("TZ", "UTC");
    expect(ciChip({ ...pr, ci: "pass" })).toEqual({ text: "CI pass", tone: "success", title: "read 14:04" });
    expect(ciChip({ ...pr, ci: "fail" })?.text).toBe("CI failed");
    expect(ciChip({ ...pr, ci: "fail" })?.tone).toBe("danger");
    expect(ciChip({ ...pr, ci: "pending" })?.text).toBe("CI pending");
    expect(ciChip({ ...pr, ci: "none" })?.text).toBe("CI none");
    const unknown = ciChip({ ...pr, ci: "unknown", ciError: "gh pr view: timed out" });
    expect(unknown?.text).toBe("CI unknown");
    expect(unknown?.title).toContain("timed out");
    expect(ciChip({ ...pr, ci: null })).toBeNull();
    expect(ciChip({ prUrl: null, ci: "pass" })).toBeNull();
    vi.unstubAllEnvs();
  });
});

describe("PR size words", () => {
  const size = { lines: 312, files: 6, excluded: 1, limit: 400, over: false, reason: null };

  it("words the badge: lines, and over the limit in text", () => {
    expect(sizeBadgeText(size)).toBe("312 lines");
    expect(sizeBadgeText({ ...size, lines: 512, over: true })).toBe("512 lines · over 400");
    // The verdict is state.sh's: more lines than the limit with over=false is shown as it is.
    expect(sizeBadgeText({ ...size, lines: 900 })).toBe("900 lines");
  });

  it("words the tooltip: files, excluded, limit, and the reason or no reason when over", () => {
    expect(sizeTitle(size)).toBe("6 files, 1 excluded, limit 400");
    expect(sizeTitle({ ...size, over: true, reason: "ships together" })).toBe("6 files, 1 excluded, limit 400; reason: ships together");
    expect(sizeTitle({ ...size, over: true })).toBe("6 files, 1 excluded, limit 400; no reason");
  });
});
