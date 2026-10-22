// @vitest-environment jsdom
import { fireEvent, render, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import fixture from "../../fixtures/run/run-view.json";
import type { RunView } from "../run/api";
import { checkSignature, wantsAppChecks } from "../run/words";
import { JanitorDialog, RunPanelView } from "./RunPanel";

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

describe("RunPanelView", () => {
  it("shows the stage, that Claude is asking in the tab, and the plan", () => {
    renderPanel();
    expect(screen.getByTestId("run-stage").textContent).toBe("Waiting for your go to open the PRs");
    expect(screen.getByText("Claude is asking you in the tab.")).toBeTruthy();
    expect(screen.getByText(/Plan ✓ approved/)).toBeTruthy();
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
