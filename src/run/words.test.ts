import { afterEach, describe, expect, it, vi } from "vitest";
import pushRs from "../../src-tauri/src/observe/push.rs?raw";
import attentionRs from "../../src-tauri/src/observe/attention.rs?raw";
import type { RunView } from "./api";
import {
  agentsFold,
  cleanupFold,
  clickupFold,
  foldForcedOpen,
  FOLD_TITLES,
  planFold,
  prsFold,
  sessionFold,
  timelineFold,
  waitingLabel,
  worstTone,
  type Tone,
  prCount,
  prFraction,
  ATTENTION_KINDS,
  checkTone,
  evalBaseText,
  evalCheckText,
  evalDistText,
  evalEstimateText,
  evalHoldReason,
  evalUsageLine,
  evalFinished,
  outcomeText,
  runFinished,
  stageText,
  compareGroupText,
  compareKeyText,
  compareOpensTrace,
  compareSentence,
  compareValueText,
  ESTIMATE_NOTE,
  ESTIMATES_CAPTION,
  orchestrationStatusText,
  orchestrationText,
  orchestrationTone,
  outcomeTone,
  tabProgress,
  attentionText,
  activityTitle,
  FAILED_TEXT,
  runsButton,
  optionsSummary,
  PREVIEW_OPENED_IN_EDITOR,
  EVAL_FINISHED_TEXT,
  ciChip,
  gateTone,
  groupTimeline,
  localTime,
  minutesText,
  planReviewTone,
  prsDone,
  prStateText,
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

describe("attention kind keys", () => {
  it("lists every Kind::ALL key of attention.rs, including eval-finished, in Settings and the phone list", () => {
    const body = /=> "([a-z-]+)",/g;
    const keyFn = /pub fn key\(self\)[\s\S]*?\n    }\n/.exec(attentionRs)?.[0] ?? "";
    const rust = [...keyFn.matchAll(body)].map((m) => m[1]).sort();
    expect(rust).toContain("eval-finished");
    expect(ATTENTION_KINDS.map((k) => k.key).sort()).toEqual(rust);
    expect(PUSH_KINDS.map((k) => k.key)).toContain("eval-finished");
    expect(ATTENTION_KINDS.map((k) => k.key)).toContain("not-started");
    expect(PUSH_KINDS.map((k) => k.key)).toContain("not-started");
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
    expect(prStateTone("qa")).toBe("running");
    expect(prStateText("qa")).toBe("trying in the app");
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

describe("tabProgress", () => {
  const base = { runId: "r-1", stage: "building", waitingFor: null, prsDone: 2, prsTotal: 3 };

  it("is null for a tab without a run or a run without a stage", () => {
    expect(tabProgress(null)).toBeNull();
    expect(tabProgress({ ...base, runId: null })).toBeNull();
    expect(tabProgress({ ...base, stage: null })).toBeNull();
  });

  it("shows PRs done of total for a running run", () => {
    const p = tabProgress(base)!;
    expect(p.fraction).toBeCloseTo(2 / 3);
    expect(p.tone).toBe("running");
    expect(p.waiting).toBe(false);
    expect(p.title).toBe("Building · 2/3 PRs");
  });

  it("falls back to the share of steps without PRs, and honours the epic-plan steps", () => {
    expect(tabProgress({ ...base, stage: "planning", prsDone: 0, prsTotal: 0 })!.fraction).toBe(0);
    expect(tabProgress({ ...base, stage: "ship-approval", prsDone: 0, prsTotal: 0 })!.fraction).toBeCloseTo(0.6);
    expect(tabProgress({ ...base, stage: "plan-approval", prsDone: 0, prsTotal: 0, runMode: "epic-plan" })!.fraction).toBe(0.5);
    expect(tabProgress({ ...base, stage: "mystery", prsDone: 0, prsTotal: 0 })!.fraction).toBe(0);
  });

  it("marks a waiting run, a done run and a failed run", () => {
    const waiting = tabProgress({ ...base, stage: "ship-approval", waitingFor: "ship-approval" })!;
    expect(waiting).toMatchObject({ tone: "you", waiting: true });
    expect(waiting.title).toBe("Waiting for you · 2/3 PRs");
    expect(tabProgress({ ...base, stage: "done", prsDone: 3 })).toMatchObject({ fraction: 1, tone: "success", title: "Done · 3/3 PRs" });
    const failed = tabProgress({ ...base, stage: "failed", prsTotal: 0 })!;
    expect(failed).toMatchObject({ fraction: 1, tone: "danger", waiting: false, title: "Failed" });
  });
});

describe("outcomeTone and the estimate words", () => {
  it("takes the outcome, else the stage", () => {
    expect(outcomeTone("done", "done")).toBe("success");
    expect(outcomeTone("failed", "failed")).toBe("danger");
    expect(outcomeTone(null, "failed")).toBe("danger");
    expect(outcomeTone(null, "building")).toBe("running");
    expect(outcomeTone(null, null)).toBe("muted");
  });

  it("keeps the one wording of an estimate", () => {
    expect(ESTIMATE_NOTE).toBe("An estimate at API list prices, not a bill");
    expect(ESTIMATES_CAPTION).toBe("Estimates at API list prices");
  });
});

describe("the run lane's words and tones", () => {
  const e = (kind: string, name: string | null, status: string | null, attrs: Record<string, unknown> = {}) => ({
    kind,
    name,
    status,
    attrs,
  });

  it("names every orchestration kind in the app's words", () => {
    const words: [ReturnType<typeof e>, string][] = [
      [e("stage.change", "reviewing", null), "Stage: reviewing"],
      [e("approval.wait", "plan", null), "Waiting for plan approval"],
      [e("approval.wait", "ship", null), "Waiting for ship approval"],
      [e("approval.decision", "plan", "ok"), "Plan approved"],
      [e("approval.decision", "ship", "declined"), "Ship declined"],
      [e("review.round", "PR2-round1", null, { pr: "PR2", round: 1 }), "PR2 review round 1"],
      [e("review.round", "plan-round2", null, { round: 2 }), "Plan review round 2"],
      [e("check.end", "cargo test", "fail", { source: "app", pr: "PR2" }), "Check (app) PR2 failed"],
      [e("check.end", "npm test", "pass", { source: "skill", pr: "PR1" }), "Check (skill) PR1 passed"],
      [e("check.end", "x", "fail", { source: "stop-check", pr: "PR2" }), "stop-check PR2 failed"],
      [e("check.end", "x", "skip", { source: "stop-check" }), "stop-check skipped"],
      [e("ci.state", "fail", "fail", { pr: "PR2", ci: "fail" }), "CI PR2 failed"],
      [e("x.dcv2.pr-state", "fixing", null, { pr: "PR2", from: "reviewing", to: "fixing" }), "pr PR2 reviewing → fixing"],
      [e("retry", "PR2", null, { pr: "PR2" }), "Retry PR2"],
      [e("attention", "approval-wait", null), "Needs attention: approval-wait"],
      [e("x.dcv2.log", null, null), "Log line"],
      [e("x.dcv2.request", "recheck", "ok", { result: "done" }), "Request: recheck done"],
      [e("tool.blocked", "Bash", "blocked"), "Guard blocked Bash"],
    ];
    for (const [event, text] of words) expect(orchestrationText(event), text).toBe(text);
    expect(orchestrationStatusText(e("check.end", "x", "fail"))).toBe("failed");
    expect(orchestrationStatusText(e("approval.decision", "plan", "ok"))).toBe("approved");
    expect(orchestrationStatusText(e("x.dcv2.log", null, null))).toBeNull();
  });

  it("gives each kind its tone: danger for failed checks, warn for blocks and retries, success for passes", () => {
    expect(orchestrationTone("check.end", "fail")).toBe("danger");
    expect(orchestrationTone("check.end", "error")).toBe("danger");
    expect(orchestrationTone("check.end", "pass")).toBe("success");
    expect(orchestrationTone("check.end", "skip")).toBe("muted");
    expect(orchestrationTone("ci.state", "fail")).toBe("danger");
    expect(orchestrationTone("ci.state", "pass")).toBe("success");
    expect(orchestrationTone("tool.blocked", "blocked")).toBe("warn");
    expect(orchestrationTone("retry", null)).toBe("warn");
    expect(orchestrationTone("approval.decision", "ok")).toBe("success");
    expect(orchestrationTone("approval.decision", "declined")).toBe("warn");
    expect(orchestrationTone("approval.wait", null)).toBe("you");
    expect(orchestrationTone("stage.change", null)).toBe("running");
    expect(orchestrationTone("x.dcv2.pr-state", null)).toBe("muted");
    expect(orchestrationTone("x.dcv2.log", null)).toBe("muted");
    expect(orchestrationTone("x.dcv2.request", "ok")).toBe("muted");
    expect(orchestrationTone("whatever", undefined)).toBe("muted");
  });
});

describe("compare words", () => {
  it("words every group, key and cell, and null is never a number", () => {
    expect(compareGroupText("windowShare")).toBe("Window share");
    expect(compareGroupText("something-new")).toBe("something-new");
    expect(compareKeyText("failures", "tool.blocked:run-file-write")).toBe("guard blocks (run-file-write)");
    expect(compareKeyText("failures", "tool.end")).toBe("failed tool calls");
    expect(compareKeyText("checks", "skill/PR2")).toBe("skill check, PR2");
    expect(compareKeyText("retries", "PR2")).toBe("retries of PR2");
    expect(compareKeyText("tokens", "cacheWrite")).toBe("cache write");
    expect(compareKeyText("windowShare", "fiveHour")).toBe("five-hour window");
    expect(compareKeyText("tools", "Bash")).toBe("Bash");
    expect(compareValueText("tools", 0)).toBe("0");
    expect(compareValueText("stages", 12.5)).toBe("12.5 min");
    expect(compareValueText("tokens", 12345)).toBe("12,345");
    expect(compareValueText("windowShare", 4.5)).toBe("4.5 points (estimate)");
    expect(compareValueText("windowShare", null)).toBe("not recorded");
    expect(compareValueText("checks", "pass")).toBe("pass");
  });

  it("states the first difference with both values, or that there is none, and only event rows open a Trace", () => {
    expect(compareSentence({ group: "failures", key: "tool.blocked", a: 1, b: 0 })).toBe(
      "First difference: Failures, guard blocks (1 vs 0)",
    );
    expect(compareSentence({ group: "windowShare", key: "weekly", a: null, b: 2 })).toContain("not recorded vs 2 points (estimate)");
    expect(compareSentence(null)).toBe("No differences in these rows.");
    expect(compareOpensTrace("failures")).toBe(true);
    expect(compareOpensTrace("tokens")).toBe(false);
    expect(compareOpensTrace("windowShare")).toBe(false);
    expect(compareOpensTrace("stages")).toBe(false);
  });
});

describe("a finished eval run", () => {
  const evalRef = { group: "b-1", index: 1, of: 3, source: "r-0" };
  const finished = { stage: "ship-approval", eval: evalRef };

  it("is finished, not waiting, with the success tone and its own words", () => {
    expect(evalFinished(finished)).toBe(true);
    expect(evalFinished({ stage: "building", eval: evalRef })).toBe(false);
    expect(evalFinished({ stage: "ship-approval", eval: null })).toBe(false);
    expect(evalFinished(null)).toBe(false);
    expect(runFinished(finished)).toBe(true);
    expect(runFinished({ stage: "ship-approval" })).toBe(false);
    expect(runFinished({ stage: "building", eval: evalRef })).toBe(false);
    const progress = tabProgress({ runId: "r-1", stage: "ship-approval", waitingFor: null, eval: evalRef, prsDone: 2, prsTotal: 2 });
    expect(progress).toEqual({ fraction: 1, tone: "success", waiting: false, title: "Eval run finished · 2/2 PRs", word: "Eval run finished" });
    // Even a stray waitingFor on an eval run at its end does not read as waiting.
    expect(tabProgress({ runId: "r-1", stage: "ship-approval", waitingFor: "ship-approval", eval: evalRef, prsDone: 0, prsTotal: 0 })?.waiting).toBe(false);
    // A normal run at ship-approval still waits.
    expect(tabProgress({ runId: "r-2", stage: "ship-approval", waitingFor: "ship-approval", prsDone: 0, prsTotal: 1 })?.waiting).toBe(true);
    expect(outcomeTone(undefined, "ship-approval", evalRef)).toBe("success");
    expect(outcomeTone("eval-done", "ship-approval")).toBe("success");
    expect(outcomeTone("failed", "failed", evalRef)).toBe("danger");
    expect(outcomeTone(undefined, "ship-approval")).toBe("running");
  });

  it("words the stage and the outcome, and leaves a normal stage as it was", () => {
    expect(stageText("ship-approval", evalRef)).toBe("Eval run finished");
    expect(stageText("ship-approval")).toBe("Waiting for your go to open the PRs");
    expect(stageText("building", evalRef)).toBe("Building");
    expect(stageText(null, evalRef)).toBe("Starting");
    expect(outcomeText("eval-done")).toBe("Eval run finished");
    expect(outcomeText("done")).toBe("Done");
    expect(outcomeText("failed")).toBe("Failed");
    expect(outcomeText(undefined)).toBe("Starting");
  });

  it("has an eval stepper that skips the ship steps and shows every step done at the end", () => {
    const steps = runSteps("ship-approval", null, { done: 1, total: 1 }, { mode: "eval", finished: true });
    expect(steps?.steps.map((s) => s.label)).toEqual(["Planning", "Plan approval", "Building", "Eval finished"]);
    expect(steps?.steps.every((s) => s.status === "done")).toBe(true);
    expect(steps?.steps.some((s) => s.you)).toBe(false);
    const running = runSteps("building", null, { done: 0, total: 1 }, { mode: "eval" });
    expect(running?.steps.map((s) => s.status)).toEqual(["done", "done", "current", "todo"]);
  });
});

describe("eval words", () => {
  const NOW = 1_800_000_000;
  it("holds like the backend: no reading with a hold set holds, a reading past its reset is clear", () => {
    const reading = (percent: number, resetsAt: number | null = NOW + 100) => ({ percent, resetsAt });
    expect(evalHoldReason(null, null, NOW)).toBeNull();
    expect(evalHoldReason(undefined, reading(99), NOW)).toBeNull();
    expect(evalHoldReason(80, null, NOW)).toContain("not known");
    expect(evalHoldReason(80, reading(95, NOW - 1), NOW)).toBeNull();
    expect(evalHoldReason(80, reading(95, NOW), NOW)).toBeNull();
    expect(evalHoldReason(80, reading(80), NOW)).toBe("Five-hour usage is 80%, at or past your hold of 80%.");
    expect(evalHoldReason(80, reading(79.4), NOW)).toBeNull();
    expect(evalHoldReason(80, reading(90, null), NOW)).toContain("90%");
  });

  it("words the usage, estimate, base and check lines", () => {
    expect(evalUsageLine(null, null, NOW)).toBe("No usage hold is set");
    expect(evalUsageLine(80, null, NOW)).toBe("Five-hour usage is not known");
    expect(evalUsageLine(80, { percent: 90, resetsAt: NOW - 5 }, NOW)).toBe("The five-hour window has reset (hold 80%)");
    expect(evalUsageLine(80, { percent: 42.4, resetsAt: NOW + 5 }, NOW)).toBe("Five-hour usage 42% · hold 80%");
    expect(evalEstimateText(3, 4)).toBe("Estimate: about 12 points of the five-hour window (3 × 4)");
    expect(evalEstimateText(3, null)).toBe("Estimate: not known");
    expect(evalEstimateText(3, undefined)).toBe("Estimate: not known");
    expect(evalBaseText(["/w/app"], [])).toBe("Builds on the source run's commits");
    expect(evalBaseText([], [])).toBe("Builds on the current default branch");
    expect(evalBaseText(["/w/app"], ["/w/docs"])).toContain("default branch for docs");
    expect(evalCheckText("checking", null)).toBe("checking…");
    expect(evalCheckText("not checked", null)).toBe("not checked");
    expect(evalCheckText("checked", 0.5)).toBe("50% pass");
    expect(evalCheckText("checked", 0)).toBe("0% pass");
  });

  it("words a distribution, and a missing one as no figures", () => {
    expect(evalDistText({ min: 1, median: 2.5, max: 4, of: 3 })).toBe("min 1 · median 2.5 · max 4 (over 3 runs)");
    expect(evalDistText({ min: 1, median: 1, max: 1, of: 1 }, (v) => `${v}x`)).toBe("min 1x · median 1x · max 1x (over 1 run)");
    expect(evalDistText({ min: null, median: null, max: null, of: 0 })).toBe("no figures");
  });
});

describe("re-run words", () => {
  it("maps every re-run state to words and a tone", async () => {
    const { rerunStateText, rerunStateTone, rerunActive, lastLines, rerunTitle, REPLAY_LABEL } = await import("./words");
    const states = ["preparing", "setup", "running", "pass", "fail", "timeout", "error", "stopped", "interrupted"];
    for (const state of states) {
      expect(rerunStateText(state), state).not.toBe("");
      expect(rerunStateText(state)).not.toBe(state === "timeout" ? "timeout" : "");
    }
    expect(states.map(rerunStateTone)).toEqual([
      "running", "running", "running", "success", "danger", "danger", "danger", "muted", "muted",
    ]);
    expect(states.filter(rerunActive)).toEqual(["preparing", "setup", "running"]);
    expect(rerunStateText("interrupted")).toBe("interrupted (the app quit)");
    expect(REPLAY_LABEL).toEqual({ recorded: "recorded", rerun: "re-run", fresh: "new execution" });
    expect(rerunTitle("PR2", "3f2a9c1abcdef")).toBe("Re-run PR2's check at 3f2a9c1");
    expect(lastLines("a\nb\nc\n", 2)).toBe("b\nc");
  });
});

describe("the PR plural rule", () => {
  it("prCount says 1 PR, 2 PRs and 0 PRs", () => {
    expect([0, 1, 2].map(prCount)).toEqual(["0 PRs", "1 PR", "2 PRs"]);
  });

  it("prFraction takes the plural from the total", () => {
    expect([prFraction(0, 1), prFraction(1, 2), prFraction(2, 2)]).toEqual(["0/1 PR", "1/2 PRs", "2/2 PRs"]);
  });

  it("no component builds a PRs suffix next to a count by hand", () => {
    const files = import.meta.glob("/src/components/*.tsx", { query: "?raw", import: "default", eager: true }) as Record<string, string>;
    const offenders = Object.entries(files)
      .filter(([path]) => !/\.test\.tsx$/.test(path))
      .flatMap(([path, text]) =>
        text
          .split("\n")
          .map((line, at) => ({ line, at: at + 1 }))
          .filter(({ line }) => /\}\s*PRs\b/.test(line) || /["'`]PRs["'`]/.test(line))
          .map(({ line, at }) => `${path}:${at}: ${line.trim()}`),
      );
    expect(offenders, "route PR counts through prCount / prFraction in src/run/words.ts").toEqual([]);
  });
});

describe("run panel fold summaries", () => {
  const pr = (over: Record<string, unknown> = {}) =>
    ({ id: "PR1", state: "building", check: { result: null }, qa: null, prUrl: null, ...over }) as unknown as RunView["prs"][number];

  it("orders tones by how much attention they need, muted for none", () => {
    expect(worstTone([])).toBe("muted");
    expect(worstTone(["success", "running"])).toBe("running");
    expect(worstTone(["success", "warn", "running"])).toBe("warn");
    expect(worstTone(["warn", "you"])).toBe("you");
    expect(worstTone(["you", "danger", "success"])).toBe("danger");
  });

  it("forces only you, warn and danger sections open", () => {
    expect(["you", "warn", "danger"].map((t) => foldForcedOpen(t as Tone))).toEqual([true, true, true]);
    expect(["success", "running", "muted"].map((t) => foldForcedOpen(t as Tone))).toEqual([false, false, false]);
  });

  it("summarises the PRs and flags a failing one", () => {
    expect(prsFold({ prs: [pr(), pr({ id: "PR2", state: "opened" })], waitingFor: null })).toEqual({ summary: "1/2 done", tone: "running" });
    const failing = prsFold({ prs: [pr({ qa: { result: "fail", tried: 1, failed: 1, total: 1, rows: [] } })], waitingFor: null });
    expect(failing.tone).toBe("danger");
    expect(failing.summary).toBe("0/1 done · something failed");
    expect(prsFold({ prs: [pr({ state: "opened" })], waitingFor: "ship-approval" }).tone).toBe("you");
    expect(prsFold({ prs: [pr({ state: "fixing" })], waitingFor: null }).tone).toBe("warn");
  });

  it("summarises the plan, with BMAD, and makes a plan awaiting approval the user's", () => {
    const plan = { approved: false, verdict: null, reviewRound: null, openFindings: null, riskVerdict: null, riskOpenFindings: null };
    expect(planFold({ plan, bmad: null, waitingFor: null })).toEqual({ summary: "being written", tone: "muted" });
    expect(planFold({ plan, bmad: null, waitingFor: "plan-approval" }).tone).toBe("you");
    expect(planFold({ plan: { ...plan, approved: true, openFindings: 0 }, bmad: { mode: "on", size: "L" } as RunView["bmad"], waitingFor: null })).toEqual({
      summary: "approved · BMAD size L",
      tone: "success",
    });
    expect(planFold({ plan: { ...plan, approved: true, openFindings: 2 }, bmad: null, waitingFor: null }).tone).toBe("warn");
    expect(planFold({ plan: null, bmad: null, waitingFor: null }).summary).toBe("no plan yet");
  });

  it("summarises cleanup, the timeline and the fixed sections", () => {
    expect(cleanupFold([{}, {}])).toEqual({ summary: "2 cleaned up", tone: "muted" });
    expect(cleanupFold([{ retry: true }, {}])).toEqual({ summary: "2 cleaned up, 1 to retry", tone: "warn" });
    expect(timelineFold([1])).toEqual({ summary: "1 event", tone: "muted" });
    expect(timelineFold([1, 2])).toEqual({ summary: "2 events", tone: "muted" });
    for (const info of [sessionFold(), agentsFold(), clickupFold()]) expect(info.tone).toBe("muted");
    expect(Object.keys(FOLD_TITLES)).toEqual(["session", "plan", "prs", "cleanup", "agents", "clickup", "timeline"]);
  });

  it("names what the collapsed rail waits for", () => {
    expect(waitingLabel("plan-approval")).toBe("Plan");
    expect(waitingLabel("ship-approval")).toBe("Ship");
    expect(waitingLabel("other")).toBe("other");
    expect(waitingLabel(null)).toBeNull();
  });
});

describe("status words", () => {
  it("words the attention control", () => {
    expect(attentionText({ needsYou: 0, activeRuns: 0 })).toBeNull();
    expect(attentionText({ needsYou: 1, activeRuns: 0 })).toBe("1 needs you");
    expect(attentionText({ needsYou: 2, activeRuns: 3 })).toBe("2 need you · 3 runs");
    expect(attentionText({ needsYou: 0, activeRuns: 1 })).toBe("1 run");
  });

  it("makes the Runs button for every attention state", () => {
    expect(runsButton(null)).toEqual({ text: "Runs", tone: "muted" });
    expect(runsButton({ needsYou: 0, activeRuns: 0 })).toEqual({ text: "Runs", tone: "muted" });
    expect(runsButton({ needsYou: 0, activeRuns: 3 })).toEqual({ text: "3 runs", tone: "running" });
    expect(runsButton({ needsYou: 2, activeRuns: 3 })).toEqual({ text: "2 need you · 3 runs", tone: "you" });
  });

  it("owns the failed word and the dot tooltips", () => {
    expect(FAILED_TEXT).toBe(stageText("failed"));
    expect(activityTitle("needs-you")).toBe("Claude needs you");
    expect(activityTitle("waiting")).toBe("Claude finished its turn");
    expect(activityTitle("working")).toBe("Claude is working");
    expect(PREVIEW_OPENED_IN_EDITOR).toBe("Opened in your editor.");
  });

  it("joins the Options summary in order", () => {
    expect(optionsSummary({ implementers: 3, reviewOnGitHub: true, epicPlan: false, bmadMode: "auto" })).toBe(
      "3 implementers · GitHub review · BMAD auto",
    );
    expect(optionsSummary({ implementers: 1, reviewOnGitHub: false, epicPlan: true, bmadMode: "on" })).toBe(
      "1 implementer · Local review · Epic waves · BMAD on",
    );
    expect(optionsSummary({ implementers: 3, reviewOnGitHub: true, epicPlan: true, bmadMode: "off" })).toBe(
      "3 implementers · GitHub review · Epic waves · BMAD off",
    );
  });
});

describe("tabProgress word", () => {
  const base = { runId: "r-1", waitingFor: null, prsDone: 0, prsTotal: 0 };
  it("is the step label of the run's step set", () => {
    expect(tabProgress({ ...base, stage: "planning" })!.word).toBe("Planning");
    expect(tabProgress({ ...base, stage: "plan-approval", waitingFor: "plan-approval" })!.word).toBe("Plan approval");
    expect(tabProgress({ ...base, stage: "building" })!.word).toBe("Building");
    expect(tabProgress({ ...base, stage: "ship-approval" })!.word).toBe("Ship approval");
    expect(tabProgress({ ...base, stage: "shipping" })!.word).toBe("Shipping");
    expect(tabProgress({ ...base, stage: "done" })!.word).toBe("Done");
  });
  it("says Failed, the finished eval words and an unknown stage as it is", () => {
    expect(tabProgress({ ...base, stage: "failed" })!.word).toBe(FAILED_TEXT);
    expect(tabProgress({ ...base, stage: "ship-approval", eval: { group: "g", index: 0 } as never })!.word).toBe(EVAL_FINISHED_TEXT);
    expect(tabProgress({ ...base, stage: "mystery" })!.word).toBe("mystery");
  });
});
