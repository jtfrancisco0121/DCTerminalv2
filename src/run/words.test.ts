import { afterEach, describe, expect, it, vi } from "vitest";
import pushRs from "../../src-tauri/src/observe/push.rs?raw";
import {
  ATTENTION_KINDS,
  checkTone,
  ciChip,
  gateTone,
  groupTimeline,
  localTime,
  minutesText,
  planReviewTone,
  prsDone,
  prStateTone,
  PUSH_KINDS,
  relativeDate,
  reachedStep,
  reviewTone,
  riskTone,
  runMatches,
  runSteps,
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

describe("run steps", () => {
  const none = { done: 0, total: 0 };
  const status = (stage: string, waitingFor: string | null = null, opts = {}) =>
    runSteps(stage, waitingFor, none, opts)?.steps.map((step) => `${step.key}:${step.status}`);

  it("lists six steps with the current one marked, for each normal stage", () => {
    expect(status("planning")).toEqual([
      "planning:current",
      "plan-approval:todo",
      "building:todo",
      "ship-approval:todo",
      "shipping:todo",
      "done:todo",
    ]);
    expect(status("building")?.slice(0, 4)).toEqual(["planning:done", "plan-approval:done", "building:current", "ship-approval:todo"]);
    expect(status("shipping")?.[4]).toBe("shipping:current");
    expect(status("done")?.every((s) => s.endsWith(":done"))).toBe(true);
    expect(runSteps("building", null, none)?.failed).toBe(false);
  });

  it("marks the step waited at as waiting, earlier ones done, later ones todo", () => {
    expect(status("plan-approval", "plan-approval")).toEqual([
      "planning:done",
      "plan-approval:waiting",
      "building:todo",
      "ship-approval:todo",
      "shipping:todo",
      "done:todo",
    ]);
    expect(status("ship-approval", "ship-approval")?.slice(2)).toEqual(["building:done", "ship-approval:waiting", "shipping:todo", "done:todo"]);
  });

  it("marks both approval steps as the user's at every stage", () => {
    for (const stage of ["planning", "plan-approval", "building", "ship-approval", "shipping", "done", "failed"]) {
      const steps = runSteps(stage, null, none)?.steps ?? [];
      expect(steps.filter((s) => s.you).map((s) => s.key), stage).toEqual(["plan-approval", "ship-approval"]);
    }
  });

  it("an epic-plan run has Planning, Plan approval and Done only", () => {
    const at = (stage: string) => runSteps(stage, null, none, { mode: "epic-plan" })?.steps.map((s) => `${s.label}:${s.status}`);
    expect(at("done")).toEqual(["Planning:done", "Plan approval:done", "Done:done"]);
    expect(at("planning")).toEqual(["Planning:current", "Plan approval:todo", "Done:todo"]);
    expect(at("building")).toBeUndefined();
    expect(runSteps("building", null, none, { mode: "something-new" })?.steps).toHaveLength(6);
  });

  it("is null for no stage or an unknown one", () => {
    expect(runSteps(null, null, none)).toBeNull();
    expect(runSteps("time-travel", null, none)).toBeNull();
  });

  it("a failed run shows the steps reached, then the failed end", () => {
    const reached = runSteps("failed", null, none, { reached: "building" });
    expect(reached?.failed).toBe(true);
    expect(reached?.steps.map((s) => s.status)).toEqual(["done", "done", "done", "todo", "todo", "todo"]);
    const unknown = runSteps("failed", null, none);
    expect(unknown?.failed).toBe(true);
    expect(unknown?.steps.every((s) => s.status === "todo")).toBe(true);
  });

  it("shows n/m PRs on Building only with PRs", () => {
    const withPrs = runSteps("building", null, { done: 2, total: 3 });
    expect(withPrs?.steps.find((s) => s.key === "building")?.extra).toBe("2/3 PRs");
    expect(withPrs?.steps.filter((s) => s.extra)).toHaveLength(1);
    expect(runSteps("building", null, none)?.steps.some((s) => s.extra)).toBe(false);
  });

  it("counts exactly the five done PR states", () => {
    const states = ["reviewed", "drafted", "opened", "pushed", "local-only", "building", "fixing", "pending", "needs-attention", null];
    expect(prsDone(states.map((state) => ({ state })))).toBe(5);
  });

  it("finds the step a run reached from the newest known-stage line", () => {
    const line = (stage: string | null) => ({ time: null, stage, text: "x" });
    expect(reachedStep([line("planning"), line("building"), line("fixing"), line(null)])).toBe("building");
    expect(reachedStep([line("fixing")])).toBeNull();
    expect(reachedStep([line("planning"), line("building")], "epic-plan")).toBe("planning");
    expect(reachedStep([])).toBeNull();
  });
});

describe("groupTimeline", () => {
  const line = (stage: string | null, n: number) => ({ time: `2026-10-27T10:${String(n).padStart(2, "0")}:00Z`, stage, text: `line ${n}` });

  it("groups consecutive stages, newest first, and a returning stage forms a new group", () => {
    const groups = groupTimeline([line("planning", 0), line("building", 1), line("building", 2), line("fixing", 3), line("building", 4), line(null, 5)]);
    expect(groups.map((g) => `${g.label}:${g.lines.length}`)).toEqual(["Other:1", "Building:1", "fixing:1", "Building:2", "Planning:1"]);
    expect(groups[3]!.lines.map((l) => l.text)).toEqual(["line 2", "line 1"]);
  });

  it("keeps only the newest 30 lines", () => {
    const lines = Array.from({ length: 40 }, (_, i) => line("building", i));
    const groups = groupTimeline(lines);
    expect(groups).toHaveLength(1);
    expect(groups[0]!.lines).toHaveLength(30);
    expect(groups[0]!.lines[0]!.text).toBe("line 39");
    expect(groupTimeline(lines, 5)[0]!.lines).toHaveLength(5);
  });

  it("keys stay the same when a line is appended to the newest group", () => {
    const before = groupTimeline([line("planning", 0), line("building", 1)]);
    const after = groupTimeline([line("planning", 0), line("building", 1), line("building", 2)]);
    expect(after.map((g) => g.key)).toEqual(before.map((g) => g.key));
    expect(new Set(after.map((g) => g.key)).size).toBe(2);
  });
});
