// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import fixture from "../../fixtures/run/run-view.json";
import type { RunView } from "../run/api";
import { checkSignature, wantsAppChecks } from "../run/words";
import { JanitorDialog, mergeKey, RunPanelView } from "./RunPanel";

// The view Rust reads from fixtures/run/r-20261022-1304 (status.rs fixture test).
const view = fixture as unknown as RunView;

function renderPanel(overrides: Partial<RunView> = {}) {
  const handlers = {
    onRerunChecks: vi.fn(),
    onOpenFile: vi.fn(),
    onOpenUrl: vi.fn(),
    onCleanUp: vi.fn(),
  };
  render(<RunPanelView view={{ ...view, ...overrides }} checking={false} {...handlers} />);
  return handlers;
}

describe("PR card actions", () => {
  const handlers = () => ({
    onRerunChecks: vi.fn(),
    onOpenFile: vi.fn(),
    onOpenUrl: vi.fn(),
    onCleanUp: vi.fn(),
    onPrAction: vi.fn(),
    onCopyBranch: vi.fn(),
  });

  it("copies the branch and acts on a verified worktree", () => {
    const h = handlers();
    render(
      <RunPanelView
        view={view}
        checking={false}
        worktrees={{ PR1: { prId: "PR1", verified: true, reason: null } }}
        {...h}
      />,
    );
    const actions = within(screen.getByTestId("actions-PR1"));
    fireEvent.click(actions.getByText("Copy branch"));
    expect(h.onCopyBranch).toHaveBeenCalledWith("dcv2/r-20261022-1304-pr1");
    fireEvent.click(actions.getByText("Reveal"));
    fireEvent.click(actions.getByText("Open in editor"));
    fireEvent.click(actions.getByText("Shell here"));
    expect(h.onPrAction.mock.calls).toEqual([
      ["PR1", "reveal"],
      ["PR1", "editor"],
      ["PR1", "shell"],
    ]);
    // A PR with no branch or worktree has no actions.
    expect(within(screen.getByTestId("actions-PR2")).queryByText("Copy branch")).toBeNull();
    expect(within(screen.getByTestId("actions-PR2")).queryByText("Reveal")).toBeNull();
  });

  it("disables the worktree actions with the reason when the worktree is not verified", () => {
    const h = handlers();
    render(
      <RunPanelView
        view={view}
        checking={false}
        worktrees={{ PR1: { prId: "PR1", verified: false, reason: "not where this run's worktrees go" } }}
        {...h}
      />,
    );
    const reveal = within(screen.getByTestId("actions-PR1")).getByText("Reveal") as HTMLButtonElement;
    expect(reveal.disabled).toBe(true);
    expect(reveal.title).toBe("not where this run's worktrees go");
    fireEvent.click(reveal);
    expect(h.onPrAction).not.toHaveBeenCalled();
    // Copying the branch needs no worktree.
    expect((within(screen.getByTestId("actions-PR1")).getByText("Copy branch") as HTMLButtonElement).disabled).toBe(false);
  });

  it("waits for the check before enabling anything", () => {
    render(<RunPanelView view={view} checking={false} {...handlers()} />);
    const reveal = within(screen.getByTestId("actions-PR1")).getByText("Reveal") as HTMLButtonElement;
    expect(reveal.disabled).toBe(true);
    expect(reveal.title).toBe("Checking the worktree…");
  });
});

describe("RunPanelView", () => {
  it("shows a draft/ready badge and a link per review-round comment", () => {
    const handlers = renderPanel();
    // PR4 in the fixture: marked ready, two review comments.
    expect(screen.getByTestId("badge-PR4").textContent).toBe("ready");
    const comments = within(screen.getByTestId("comments-PR4"));
    fireEvent.click(comments.getByText("round 2"));
    expect(handlers.onOpenUrl).toHaveBeenCalledWith("https://github.com/o/mathlib/pull/7#issuecomment-102");
    expect(screen.queryByTestId("badge-PR1")).toBeNull();
  });

  it("draws state, checks, CI and gate of a PR row as toned pills", () => {
    const base = view.prs[0];
    const prs = [
      {
        ...base,
        id: "PR1",
        state: "reviewed",
        prUrl: "https://github.com/o/mathlib/pull/9",
        ci: "fail",
        checkKnown: true,
        check: { ...base.check, result: "pass" },
        appCheck: { ...(base.appCheck ?? ({} as never)), result: "fail" },
        gateStatus: { kind: "fail", text: "FAIL" },
      },
      { ...base, id: "PR2", state: "time-travel", gateStatus: null, review: { round: null, verdict: null, openFindings: null } },
    ] as RunView["prs"];
    renderPanel({ prs });
    const one = within(screen.getByTestId("pr-PR1"));
    expect(one.getByText("reviewed").className).toContain("pill tone-success");
    expect(one.getByText(/check \(Claude\)/).className).toContain("tone-success");
    expect(one.getByText(/check \(app\)/).className).toContain("tone-danger");
    expect(screen.getByTestId("ci-PR1").className).toContain("pill tone-danger");
    expect(screen.getByTestId("gate-PR1").className).toContain("pill tone-danger");
    const two = within(screen.getByTestId("pr-PR2")).getByText("time-travel");
    expect(two.className).toContain("pill tone-muted");
  });

  it("names a drafted PR a draft and links it from the first draft", () => {
    const prs = view.prs.map((pr) =>
      pr.id === "PR1"
        ? { ...pr, state: "drafted", draft: true, prUrl: "https://github.com/o/mathlib/pull/9", comments: [{ round: 1, url: "javascript:alert(1)" }] }
        : pr,
    );
    const handlers = renderPanel({ prs });
    const pr1 = within(screen.getByTestId("pr-PR1"));
    expect(screen.getByTestId("badge-PR1").textContent).toBe("draft");
    expect(pr1.getByText("draft PR open")).toBeTruthy();
    expect(screen.queryByTestId("comments-PR1")).toBeNull();
    fireEvent.click(pr1.getByText("Open draft PR"));
    expect(handlers.onOpenUrl).toHaveBeenCalledWith("https://github.com/o/mathlib/pull/9");
  });

  it("shows the BMAD badge, size, reasons and a link per document", () => {
    const bmad = {
      mode: "auto",
      version: "bda3c59 6.13.0-next",
      size: "Large",
      reasons: ["more than one repo", "new data model"],
      applied: true,
      checkResult: "pass",
      documents: ["bmad/output/r-1/spec-a/spec-a.md", "bmad/output/r-1/architecture-a/architecture-a.md", "bmad/output/r-1/ux-a/DESIGN.md"],
    };
    const handlers = renderPanel({ bmad });
    expect(screen.getByTestId("bmad-badge").textContent).toBe("BMAD bda3c59 6.13.0-next");
    expect(screen.getByTestId("bmad-size").textContent).toBe("size Large");
    expect(within(screen.getByTestId("bmad-reasons")).getByText("new data model")).toBeTruthy();
    const docs = within(screen.getByTestId("bmad-docs"));
    fireEvent.click(docs.getByText("Spec"));
    expect(handlers.onOpenFile).toHaveBeenCalledWith("bmad/output/r-1/spec-a/spec-a.md", "Spec");
    fireEvent.click(docs.getByText("Architecture"));
    expect(handlers.onOpenFile).toHaveBeenCalledWith("bmad/output/r-1/architecture-a/architecture-a.md", "Architecture");
    expect(docs.getByText("UX design")).toBeTruthy();
  });

  it("shows no document links while planning, and the sizing before a size exists", () => {
    renderPanel({
      stage: "planning",
      bmad: { mode: "on", version: null, size: null, reasons: [], applied: false, checkResult: null, documents: ["bmad/output/r-1/spec-a/spec-a.md"] },
    });
    expect(screen.getByTestId("bmad-badge").textContent).toBe("BMAD");
    expect(screen.getByTestId("bmad-size").textContent).toBe("on");
    expect(screen.queryByTestId("bmad-docs")).toBeNull();
  });

  it("still links the BMAD documents on a failed run", () => {
    renderPanel({
      stage: "failed",
      bmad: { mode: "on", version: null, size: "Large", reasons: [], applied: true, checkResult: "pass", documents: ["bmad/output/r-1/spec-a/spec-a.md"] },
    });
    expect(within(screen.getByTestId("bmad-docs")).getByText("Spec")).toBeTruthy();
  });

  it("shows what the app cleaned up for merged PRs", () => {
    renderPanel({
      cleanup: [
        { prId: "PR1", prUrl: "https://github.com/o/r/pull/1", outcome: "cleaned", note: "", worktree: "worktree removed", branch: "branch dcv2/r-1-pr1 deleted", remote: "remote branch deleted", at: "t" },
        { prId: "PR2", prUrl: "https://github.com/o/r/pull/2", outcome: "untouched", note: "untouched: closed without merging", worktree: "", branch: "", remote: "", at: "t" },
        { prId: "PR3", prUrl: "https://github.com/o/r/pull/3", outcome: "kept", retry: true, worktree: "worktree kept (kept: 1 changed files)", branch: "", remote: "remote branch deleted", at: "t" },
      ],
    });
    expect(screen.getByTestId("cleanup-PR1").textContent).toBe("PR1 worktree removed; branch dcv2/r-1-pr1 deleted; remote branch deleted");
    expect(screen.getByTestId("cleanup-PR2").textContent).toBe("PR2 untouched: closed without merging");
    expect(screen.getByTestId("cleanup-PR3").textContent).toMatch(/\(the app tries again later\)$/);
    cleanup();
    renderPanel({ cleanup: [] });
    expect(screen.queryByTestId("cleanup")).toBeNull();
  });

  it("shows nothing about BMAD for an old run or one with it off", () => {
    renderPanel();
    expect(screen.queryByTestId("bmad")).toBeNull();
    cleanup();
    renderPanel({ bmad: null });
    expect(screen.queryByTestId("bmad")).toBeNull();
    cleanup();
    renderPanel({ bmad: { mode: "off", version: null, size: null, reasons: [], applied: false, checkResult: null, documents: [] } });
    expect(screen.queryByTestId("bmad")).toBeNull();
  });

  it("shows the stepper beside the stage, and only the sentence for an unknown stage", () => {
    renderPanel({ stage: "building", waitingFor: null });
    expect(screen.getByRole("list", { name: "Run steps" })).toBeTruthy();
    expect(screen.getByTestId("run-stage").textContent).toBe("Building");
    cleanup();
    renderPanel({ stage: "time-travel", waitingFor: null });
    expect(screen.queryByRole("list", { name: "Run steps" })).toBeNull();
    expect(screen.getByTestId("run-stage").textContent).toBe("time-travel");
    cleanup();
    renderPanel({ stage: null, waitingFor: null });
    expect(screen.queryByRole("list", { name: "Run steps" })).toBeNull();
    expect(screen.getByTestId("run-stage").textContent).toBe("Starting");
  });

  it("a failed run shows the step it reached from the log", () => {
    renderPanel({
      stage: "failed",
      waitingFor: null,
      timeline: [
        { time: null, stage: "planning", text: "a" },
        { time: null, stage: "building", text: "b" },
        { time: null, stage: "failed", text: "c" },
      ],
    });
    expect(screen.getByTitle("Building").className).toContain("step-done");
    expect(screen.getByTitle("Ship approval (your step)").className).toContain("step-todo");
    expect(screen.getByText("Failed", { selector: ".step-label" })).toBeTruthy();
  });

  it("an epic-plan run at done has three steps", () => {
    renderPanel({ stage: "done", waitingFor: null, runMode: "epic-plan" });
    expect(screen.getByRole("list", { name: "Run steps" }).querySelectorAll("li")).toHaveLength(3);
  });

  it("shows the stage, the approval card, and the plan", () => {
    renderPanel();
    expect(screen.getByTestId("run-stage").textContent).toBe("Waiting for your go to open the PRs");
    expect(screen.getByTestId("approval-title").textContent).toBe("Ship approval");
    expect(screen.getByText(/Plan ✓ approved/)).toBeTruthy();
  });

  it("the approval card shows the risk review verdict and open risks", () => {
    renderPanel({
      waitingFor: "plan-approval",
      plan: {
        approved: false,
        verdict: "APPROVED",
        reviewRound: 5,
        openFindings: 0,
        riskVerdict: "RISKS FOUND",
        riskOpenFindings: 2,
      },
    });
    expect(screen.getByTestId("approval-prs").textContent).toBe(`${view.prs.length} PRs`);
    const plan = screen.getByTestId("approval-plan-badge");
    expect(plan.textContent).toBe("Plan review: round 5 · 0 open");
    expect(plan.className).toContain("tone-success");
    const risk = screen.getByTestId("approval-risk-badge");
    expect(risk.textContent).toBe("Risk: RISKS FOUND · 2 open");
    expect(risk.className).toContain("tone-warn");
  });

  it("the approval card says no blocking risks without a count", () => {
    renderPanel({
      waitingFor: "plan-approval",
      plan: {
        approved: false,
        verdict: "APPROVED",
        reviewRound: 1,
        openFindings: 0,
        riskVerdict: "NO BLOCKING RISKS",
        riskOpenFindings: 0,
      },
    });
    expect(screen.getByTestId("approval-prs").textContent).toBe(`${view.prs.length} PRs`);
    const risk = screen.getByTestId("approval-risk-badge");
    expect(risk.textContent).toBe("Risk: NO BLOCKING RISKS · 0 open");
    expect(risk.className).toContain("tone-success");
  });

  it("the approval card shows the PR count, each PR's gate, app check and open findings, and the plan review", () => {
    renderPanel({
      waitingFor: "plan-approval",
      plan: {
        approved: false,
        verdict: "APPROVED WITH CHANGES",
        reviewRound: 2,
        openFindings: 1,
        riskVerdict: null,
        riskOpenFindings: null,
      },
    });
    expect(screen.getByTestId("approval-title").textContent).toBe("Plan approval");
    expect(screen.getByTestId("approval-prs").textContent).toBe(`${view.prs.length} PRs`);
    const plan = screen.getByTestId("approval-plan-badge");
    expect(plan.textContent).toBe("Plan review: round 2 · 1 open");
    expect(plan.className).toContain("tone-warn");
    expect(screen.queryByTestId("approval-risk-badge")).toBeNull();
    const row = within(screen.getByTestId("approval-PR2"));
    expect(row.getByText(/✗ fail/).className).toContain("pill");
    expect(row.getAllByRole("cell")).toHaveLength(5);
    expect(screen.queryByRole("button", { name: /approve|yes|go ahead/i })).toBeNull();
  });

  it("Read plan opens the plan, Go to prompt focuses the terminal, and nothing is typed", () => {
    const onOpenFile = vi.fn();
    const onGoToPrompt = vi.fn();
    render(
      <RunPanelView
        view={view}
        checking={false}
        onRerunChecks={vi.fn()}
        onOpenFile={onOpenFile}
        onOpenUrl={vi.fn()}
        onCleanUp={vi.fn()}
        onGoToPrompt={onGoToPrompt}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: "Read plan" }));
    expect(onOpenFile).toHaveBeenCalledWith("plan.md", "Plan");
    fireEvent.click(screen.getByRole("button", { name: "Go to prompt" }));
    expect(onGoToPrompt).toHaveBeenCalledOnce();
  });

  it("an unknown approval value is shown as it is, with Go to prompt only", () => {
    renderPanel({ waitingFor: "deploy-approval" });
    expect(screen.getByTestId("approval-title").textContent).toBe("deploy-approval");
    expect(screen.queryByRole("button", { name: "Read plan" })).toBeNull();
    expect(screen.queryByTestId("approval-prs")).toBeNull();
    expect(screen.getByRole("button", { name: "Go to prompt" })).toBeTruthy();
  });

  it("shows each PR with repo, state, stories, both checks, review and gate", () => {
    renderPanel();
    const pr1 = within(screen.getByTestId("pr-PR1"));
    expect(pr1.getByText("mathlib")).toBeTruthy();
    expect(pr1.getByText("reviewed")).toBeTruthy();
    expect(pr1.getByText("2/2 stories")).toBeTruthy();
    expect(pr1.getByText(/check \(Claude\): ✓ pass/)).toBeTruthy();
    expect(pr1.getByText(/check \(app\): ✓ pass/)).toBeTruthy();
    expect(pr1.getByText("review round 1, APPROVED")).toBeTruthy();
    // HEAD of the fixture's worktree is unknown: claimed, never green.
    expect(screen.getByTestId("gate-PR1").className).toContain("run-gate-claimed");

    // WAIVED by the skill, but the app's own check failed: only claimed.
    const gate2 = screen.getByTestId("gate-PR2");
    expect(gate2.className).toContain("run-gate-claimed");
    expect(gate2.textContent).toContain("flaky e2e waived");
    expect(within(screen.getByTestId("pr-PR2")).getByText(/check \(app\): ✗ fail/)).toBeTruthy();
    // status.json's command differs from repos.json's: both shown.
    expect(screen.getByTestId("check-differs-PR2").textContent).toBe(
      "Claude ran npm test; the app runs npm run check",
    );
    expect(screen.queryByTestId("check-differs-PR1")).toBeNull();

    // No check in repos.json: review only, neutral.
    expect(screen.getByTestId("gate-PR5").className).toContain("run-gate-review-only");
    expect(screen.getByTestId("gate-PR5").textContent).toBe("PASS — review only (no check exists)");
    expect(within(screen.getByTestId("pr-PR5")).getByText("check (app): no check known")).toBeTruthy();

    // An unreadable gate is shown, not dropped.
    expect(screen.getByTestId("gate-PR3").className).toContain("run-gate-unreadable");
    expect(within(screen.getByTestId("pr-PR3")).getByText(/2 MEDIUM findings still open/)).toBeTruthy();
    expect(within(screen.getByTestId("pr-PR3")).getByText("needs attention")).toBeTruthy();
    expect(screen.queryByTestId("gate-PR4")).toBeNull();
  });

  it("opens PR links, the report and check output, and re-runs checks", () => {
    const handlers = renderPanel();
    fireEvent.click(within(screen.getByTestId("pr-PR4")).getByText("Open PR"));
    expect(handlers.onOpenUrl).toHaveBeenCalledWith("https://github.com/o/mathlib/pull/7");
    fireEvent.click(screen.getByText("Report"));
    expect(handlers.onOpenFile).toHaveBeenCalledWith("report.md", "Run report");
    fireEvent.click(within(screen.getByTestId("pr-PR2")).getByText("output"));
    expect(handlers.onOpenFile).toHaveBeenCalledWith("logs/PR2-app-check.txt", "PR2 app check");
    fireEvent.click(screen.getByText("Re-run checks"));
    expect(handlers.onRerunChecks).toHaveBeenCalled();
    expect(screen.getByText("Stop and clean up…")).toBeTruthy();
  });

  it("shows the timeline newest first", () => {
    renderPanel();
    const items = screen.getByLabelText("Timeline").querySelectorAll("li");
    expect(items.length).toBe(3);
    expect(items[0].textContent).toContain("waiting for ship approval");
  });

  it("groups the timeline by stage: the newest group open, older ones closed, the user's choice kept", () => {
    const line = (stage: string, n: number) => ({ time: `2026-10-27T10:0${n}:00Z`, stage, text: `line ${n}` });
    const timeline = [line("planning", 1), line("planning", 2), line("building", 3)];
    const { rerender } = render(
      <RunPanelView view={{ ...view, timeline }} checking={false} onRerunChecks={vi.fn()} onOpenFile={vi.fn()} onOpenUrl={vi.fn()} onCleanUp={vi.fn()} />,
    );
    const groups = () => Array.from(screen.getByLabelText("Timeline").querySelectorAll("details"));
    expect(groups().map((g) => g.open)).toEqual([true, false]);
    expect(groups().map((g) => g.querySelector("summary")?.textContent)).toEqual(["Building · 1 line", "Planning · 2 lines"]);
    groups()[1]!.open = true;
    rerender(
      <RunPanelView
        view={{ ...view, timeline: [...timeline, line("building", 4)] }}
        checking={false}
        onRerunChecks={vi.fn()}
        onOpenFile={vi.fn()}
        onOpenUrl={vi.fn()}
        onCleanUp={vi.fn()}
      />,
    );
    expect(groups().map((g) => g.open)).toEqual([true, true]);
    expect(groups()[0]!.querySelector("summary")?.textContent).toBe("Building · 2 lines");
  });

  it("puts lines without a stage under Other", () => {
    renderPanel({ timeline: [{ time: null, stage: null, text: "stray" }] });
    expect(screen.getByLabelText("Timeline").querySelector("summary")?.textContent).toBe("Other · 1 line");
  });

  it("lists the lines since you were away with local times, at most 10, a count of the rest and Dismiss", () => {
    vi.stubEnv("TZ", "Asia/Manila");
    try {
      const lines = Array.from({ length: 13 }, (_, i) => ({
        time: `2026-10-27T14:${String(i).padStart(2, "0")}:00Z`,
        stage: "building",
        text: `step ${i}`,
      }));
      const dismiss = vi.fn();
      render(
        <RunPanelView
          view={view}
          checking={false}
          onRerunChecks={vi.fn()}
          onOpenFile={vi.fn()}
          onOpenUrl={vi.fn()}
          onCleanUp={vi.fn()}
          away={{ lines, dismiss }}
        />,
      );
      const notice = screen.getByTestId("away-notice");
      expect(within(notice).getByText("While you were away")).toBeTruthy();
      const rows = notice.querySelectorAll("li");
      expect(rows).toHaveLength(10);
      expect(rows[9].textContent).toContain("step 12");
      expect(rows[9].querySelector(".run-time")?.textContent).toBe("22:12");
      expect(rows[9].querySelector(".run-time")?.getAttribute("title")).toContain("(UTC 2026-10-27T14:12:00Z)");
      expect(within(notice).getByText("+3 more")).toBeTruthy();
      fireEvent.click(within(notice).getByText("Dismiss"));
      expect(dismiss).toHaveBeenCalledOnce();
    } finally {
      vi.unstubAllEnvs();
    }
  });

  it("shows no notice when nothing changed", () => {
    renderPanel();
    expect(screen.queryByTestId("away-notice")).toBeNull();
  });

  it("shows a CI chip beside the app check only with a PR URL and a reading", () => {
    const prs = view.prs.map((pr, i) => ({
      ...pr,
      prUrl: i === 0 ? "https://github.com/o/r/pull/1" : pr.prUrl,
      ci: i === 0 ? "fail" : null,
      ciAt: "2026-10-27T14:04:27Z",
    }));
    renderPanel({ prs });
    expect(screen.getByTestId(`ci-${prs[0].id}`).textContent).toBe("CI failed");
    expect(screen.getByTestId(`ci-${prs[0].id}`).className).toContain("ci-chip-danger");
    expect(screen.queryByTestId(`ci-${prs[1].id}`)).toBeNull();
  });

  it("builds a new merge key when a PR flips to MERGED or is cleaned, with the stage unchanged", () => {
    const open: RunView = { ...view, prs: view.prs.map((pr) => ({ ...pr, prState: "OPEN" })), cleanup: [] };
    const merged: RunView = { ...open, prs: open.prs.map((pr, i) => (i === 0 ? { ...pr, prState: "MERGED" } : pr)) };
    expect(merged.stage).toBe(open.stage);
    expect(merged.updatedAt).toBe(open.updatedAt);
    expect(mergeKey(merged)).not.toBe(mergeKey(open));
    expect(mergeKey(open)).toBe(mergeKey({ ...open }));
    const first = open.prs[0].id;
    expect(mergeKey(merged)).toContain(`${first}:MERGED:`);
    const cleaned: RunView = {
      ...open,
      cleanup: [{ prId: first, prUrl: "u", outcome: "cleaned", worktree: "", branch: "", remote: "", at: "t" }],
    };
    expect(mergeKey(cleaned)).toContain(`${first}:OPEN:cleaned`);
  });

  it("shows timeline times in the local zone with the UTC time on hover", () => {
    vi.stubEnv("TZ", "Asia/Manila");
    try {
      renderPanel({
        timeline: [
          { time: "2026-10-27T14:04:27Z", stage: null, text: "built" },
          { time: null, stage: null, text: "no time" },
          { time: "garbage-time-value", stage: null, text: "bad time" },
        ],
      });
      const items = screen.getByLabelText("Timeline").querySelectorAll("li");
      // Newest first: the last line comes first.
      expect(items[2].querySelector(".run-time")?.textContent).toBe("22:04");
      expect(items[2].querySelector(".run-time")?.getAttribute("title")).toContain("(UTC 2026-10-27T14:04:27Z)");
      expect(items[1].querySelector(".run-time")).toBeNull();
      expect(items[0].querySelector(".run-time")?.textContent).toBe("garbage-time-value".slice(11, 16));
    } finally {
      vi.unstubAllEnvs();
    }
  });

  it("tolerates a run with no status yet and unknown values", () => {
    renderPanel({
      hasStatus: false,
      stage: null,
      waitingFor: null,
      plan: null,
      prs: [],
      timeline: [],
      hasReport: false,
    });
    expect(screen.getByTestId("run-stage").textContent).toBe("Starting");
    expect(screen.getByText("Claude is starting the run in the tab.")).toBeTruthy();
    expect(screen.queryByText("Re-run checks")).toBeNull();
    expect(screen.queryByText("Report")).toBeNull();
  });

  it("offers clean-up once the run is done and shows unknown stages as they are", () => {
    renderPanel({ stage: "done", waitingFor: null });
    expect(screen.getByText("Clean up worktrees")).toBeTruthy();
  });

  it("says why status.json could not be used", () => {
    renderPanel({ stage: "time-travel", statusProblem: "status.json has schema 2; this app knows schema 1." });
    expect(screen.getByTestId("run-stage").textContent).toBe("time-travel");
    expect(screen.getByText(/schema 2/)).toBeTruthy();
  });

  it("shows green only for a proven gate", () => {
    const prs = view.prs.map((pr) =>
      pr.id === "PR1" ? { ...pr, gateStatus: { kind: "proven", text: "PASS, and the app's check passed" } } : pr,
    );
    renderPanel({ prs });
    expect(screen.getByTestId("gate-PR1").className).toContain("run-gate-proven");
    expect(screen.getByTestId("gate-PR1").textContent).toBe("✓ PASS, and the app's check passed");
  });

  it("re-runs checks on its own at stop #2 for known checks with no result or a moved HEAD", () => {
    const ran = { ...view, prs: view.prs.map((pr) => ({ ...pr, appCheck: pr.appCheck ?? ({ result: "pass" } as never) })) };
    expect(wantsAppChecks(ran)).toBe(false);
    // PR1 has a known check (by its repo path) and no app result.
    const noResult = { ...ran, prs: ran.prs.map((pr) => (pr.id === "PR1" ? { ...pr, appCheck: null } : pr)) };
    expect(wantsAppChecks(noResult)).toBe(true);
    const moved = { ...ran, prs: ran.prs.map((pr) => (pr.id === "PR1" ? { ...pr, appCheckStale: true } : pr)) };
    expect(wantsAppChecks(moved)).toBe(true);
    expect(wantsAppChecks({ ...moved, stage: "building" })).toBe(false);
    const unknownOnly = { ...ran, prs: ran.prs.filter((pr) => !pr.checkKnown).map((pr) => ({ ...pr, appCheck: null })) };
    expect(wantsAppChecks(unknownOnly)).toBe(false);
    expect(checkSignature(view)).toContain("PR1@-");
  });

  it("shows the Windows shell fallback note first in the tooltip and keeps the pass tone", () => {
    const note = "ran with cmd /C (Git Bash not found)";
    const prs = view.prs.map((pr) =>
      pr.id === "PR1" ? { ...pr, appCheck: { ...(pr.appCheck ?? ({} as never)), result: "pass", detail: note } } : pr,
    );
    renderPanel({ prs });
    const pill = within(screen.getByTestId("pr-PR1")).getByText(/check \(app\): .* pass/);
    expect(pill.getAttribute("title")).toBe(note);
    expect(pill.className).toContain("tone-success");
  });

  it("marks a check that went stale", () => {
    const prs = view.prs.map((pr) => (pr.id === "PR1" ? { ...pr, appCheckStale: true } : pr));
    renderPanel({ prs });
    expect(within(screen.getByTestId("pr-PR1")).getByText(/code changed since/)).toBeTruthy();
  });
});

describe("JanitorDialog", () => {
  it("lists clean and dirty worktrees and removes only the clean ones after a confirm", () => {
    const onRemove = vi.fn();
    render(
      <JanitorDialog
        finished
        results={null}
        onRemove={onRemove}
        onClose={() => {}}
        items={[
          { prId: "PR1", worktree: "/w/a", repoPath: "/w/r", branch: "b1", exists: true, clean: true, changedFiles: 0, removable: true, problem: null },
          { prId: "PR2", worktree: "/w/b", repoPath: "/w/r", branch: "b2", exists: true, clean: false, changedFiles: 3, removable: false, problem: null },
        ]}
      />,
    );
    expect(screen.getByText(/3 changed files, kept/)).toBeTruthy();
    fireEvent.click(screen.getByText("Remove 1 clean worktree"));
    expect(onRemove).not.toHaveBeenCalled();
    fireEvent.click(screen.getByText("Remove"));
    expect(onRemove).toHaveBeenCalledWith(["PR1"]);
  });
});
