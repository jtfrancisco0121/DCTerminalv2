import { describe, expect, it } from "vitest";
import { activityText, checkLogPr, costText, pollsActivity, tokensText, urgentActivity, versionNote, versionWarning } from "./observe";

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

describe("urgentActivity", () => {
  it("picks needs-you over working over waiting, and nothing for unknown panes", () => {
    expect(urgentActivity(["waiting", "needs-you"])).toBe("needs-you");
    expect(urgentActivity(["waiting", "working"])).toBe("working");
    expect(urgentActivity([undefined, "waiting"])).toBe("waiting");
    expect(urgentActivity(["unknown", undefined])).toBeUndefined();
    expect(urgentActivity([])).toBeUndefined();
  });
});

describe("version note after a passing self-test", () => {
  const test = { version: "2.1.300", ok: true, pluginLoaded: true, agentSpawned: true, agentCompleted: true, finished: true, at: "", detail: null, seconds: 20 };
  const base = { version: "2.1.300 (Claude Code)", range: "above", min: "2.1.287", maxTested: "2.1.287", selftest: test, selftestWanted: false };

  it("a passing self-test on this version gives the note and no warning", () => {
    expect(versionWarning(base)).toBeNull();
    expect(versionNote(base)).toBe("Claude Code 2.1.300 is newer than tested · self-test passed.");
  });

  it("no self-test, a failed one or one on another version keeps the warning", () => {
    for (const selftest of [null, { ...test, ok: false }, { ...test, version: "2.1.290" }]) {
      const status = { ...base, selftest };
      expect(versionWarning(status), JSON.stringify(selftest)).toContain("newer than 2.1.287");
      expect(versionNote(status)).toBeNull();
    }
  });

  it("below is unchanged", () => {
    const status = { ...base, range: "below" };
    expect(versionWarning(status)).toContain("older than");
    expect(versionNote(status)).toBeNull();
  });
});
