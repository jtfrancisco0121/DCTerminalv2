import { afterEach, describe, expect, it, vi } from "vitest";
import pushRs from "../../src-tauri/src/observe/push.rs?raw";
import {
  ATTENTION_KINDS,
  checkTone,
  ciChip,
  gateTone,
  localTime,
  minutesText,
  planReviewTone,
  prStateTone,
  PUSH_KINDS,
  relativeDate,
  reviewTone,
  riskTone,
  runMatches,
  usageTone,
} from "./words";

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

describe("tones", () => {
  it("maps known values", () => {
    expect(checkTone("pass")).toBe("success");
    expect(["fail", "error", "timeout"].map(checkTone)).toEqual(["danger", "danger", "danger"]);
    expect(checkTone("running")).toBe("running");
    expect(checkTone("interrupted")).toBe("warn");
    for (const state of ["reviewed", "opened", "drafted", "pushed", "local-only"]) expect(prStateTone(state)).toBe("success");
    expect(prStateTone("building")).toBe("running");
    expect(prStateTone("reviewing")).toBe("running");
    expect(prStateTone("fixing")).toBe("warn");
    expect(prStateTone("needs-attention")).toBe("danger");
    expect(prStateTone("pending")).toBe("muted");
    expect(gateTone("proven")).toBe("success");
    for (const kind of ["review-only", "claimed", "concerns", "unreadable"]) expect(gateTone(kind)).toBe("warn");
    expect(gateTone("fail")).toBe("danger");
    expect(gateTone("none")).toBe("muted");
    expect(reviewTone({ round: 1, verdict: "APPROVED", openFindings: 2 })).toBe("warn");
    expect(reviewTone({ round: 1, verdict: "APPROVED", openFindings: 0 })).toBe("success");
    expect(usageTone("ok")).toBe("success");
    expect(usageTone("warn")).toBe("warn");
    expect(usageTone("hot")).toBe("danger");
    expect(usageTone("muted")).toBe("muted");
  });

  it("maps null, undefined, empty and unknown values to muted without throwing", () => {
    for (const bad of [null, undefined, "", "time-travel"]) {
      expect(checkTone(bad)).toBe("muted");
      expect(prStateTone(bad)).toBe("muted");
      expect(gateTone(bad)).toBe("muted");
      expect(usageTone(bad as never)).toBe("muted");
    }
    expect(reviewTone(null)).toBe("muted");
    expect(reviewTone(undefined)).toBe("muted");
    expect(reviewTone({ round: null, verdict: null, openFindings: null })).toBe("muted");
  });
});

describe("approval tones", () => {
  it("tones the plan review and the risk verdict", () => {
    expect(planReviewTone(0)).toBe("success");
    expect(planReviewTone(2)).toBe("warn");
    expect(planReviewTone(null)).toBe("muted");
    expect(riskTone("NO BLOCKING RISKS")).toBe("success");
    expect(riskTone("RISKS FOUND")).toBe("warn");
    for (const bad of [null, undefined, "", "MAYBE"]) expect(riskTone(bad)).toBe("muted");
  });
});
