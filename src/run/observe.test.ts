import { describe, expect, it } from "vitest";
import { activityText, checkLogPr, costText, pollsActivity, tokensText, versionWarning } from "./observe";

describe("pollsActivity", () => {
  it("polls while any Claude tab exists, even with no run tab", () => {
    expect(pollsActivity([{ kind: "claude" }])).toBe(true);
    expect(pollsActivity([{ kind: "shell", role: "eagle-eye" }])).toBe(true);
    expect(pollsActivity([{ kind: "shell" }])).toBe(false);
    expect(pollsActivity([])).toBe(false);
  });
});

describe("observe words", () => {
  it("formats tokens, cost and activity", () => {
    expect(tokensText(950)).toBe("950");
    expect(tokensText(12_400)).toBe("12k");
    expect(tokensText(2_340_000)).toBe("2.3M");
    expect(costText(null)).toBe("$ ?");
    expect(costText(0.004)).toBe("<$0.01");
    expect(costText(3.456)).toBe("$3.46");
    expect(activityText("needs-you")).toBe("needs you");
    expect(activityText("unknown")).toBeNull();
  });

  it("finds the PR of an app check log and words the version range", () => {
    expect(checkLogPr("logs/PR1-app-check.txt")).toBe("PR1");
    expect(checkLogPr("report.md")).toBeNull();
    const base = { version: "2.1.200", min: "2.1.287", maxTested: "2.1.287", selftest: null, selftestWanted: false };
    expect(versionWarning({ ...base, range: "below" })).toContain("older than 2.1.287");
    expect(versionWarning({ ...base, range: "ok" })).toBeNull();
    expect(versionWarning({ ...base, range: "unknown" })).toBeNull();
  });
});
