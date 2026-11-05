// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Dist, EvalResults as Results, EvalRunResult } from "../flight/types";

const flightEvalResults = vi.fn();
const evalCleanUp = vi.fn();
const evalDeleteBranch = vi.fn();
vi.mock("../flight/api", () => ({ flightEvalResults: (...args: unknown[]) => flightEvalResults(...args) }));
vi.mock("../run/api", () => ({
  evalCleanUp: (...args: unknown[]) => evalCleanUp(...args),
  evalDeleteBranch: (...args: unknown[]) => evalDeleteBranch(...args),
}));

const { EvalResults } = await import("./EvalResults");

const dist = (min: number | null, median: number | null, max: number | null, of = 2): Dist => ({ min, median, max, of });
const run = (index: number, over: Partial<EvalRunResult> = {}): EvalRunResult => ({
  index,
  label: `Eval run ${index} of 3`,
  runId: `r-${index}`,
  runDir: `/d/runs/r-${index}`,
  status: "finished",
  completed: true,
  checkPassRate: 1,
  checkState: "checked",
  retries: 1,
  toolFailures: 2,
  tokens: 1500,
  activeMinutes: 30,
  keptBranches: [],
  ...over,
});
const results = (over: Partial<Results> = {}): Results => ({
  group: "b-1",
  source: "r-0",
  n: 3,
  runs: [
    run(1),
    run(2, { completed: false, checkPassRate: null, checkState: "not checked", retries: 3, toolFailures: 0, tokens: 2500, activeMinutes: 50 }),
    run(3, { status: "running", completed: null, checkPassRate: null, retries: null, toolFailures: null, tokens: null, activeMinutes: null, checkState: "not checked" }),
  ],
  distributions: {
    completed: dist(0, 0.5, 1),
    checkPassRate: dist(1, 1, 1, 1),
    retries: dist(1, 2, 3),
    toolFailures: dist(0, 1, 2),
    tokens: dist(1500, 2000, 2500),
    activeMinutes: dist(30, 40, 50),
  },
  finished: 2,
  running: 1,
  failed: 0,
  ...over,
});

const noop = () => {};
const open = async (over: Partial<Results> = {}, props: Partial<Parameters<typeof EvalResults>[0]> = {}) => {
  flightEvalResults.mockResolvedValue(results(over));
  render(<EvalResults group="b-1" onClose={noop} {...props} />);
  await screen.findByRole("table", { name: "Eval runs" });
};

beforeEach(() => {
  flightEvalResults.mockReset();
  evalCleanUp.mockReset();
  evalDeleteBranch.mockReset();
});
afterEach(cleanup);

describe("EvalResults", () => {
  it("shows the six measures as distributions with text alternatives, over the finished runs only", async () => {
    await open();
    expect(screen.getByTestId("eval-counts").textContent).toContain("2 of 3 finished · 1 still going");
    for (const key of ["completed", "checkPassRate", "retries", "toolFailures", "tokens", "activeMinutes"]) {
      expect(screen.getByTestId(`strip-${key}`)).toBeTruthy();
    }
    expect(screen.getByTestId("strip-text-retries").textContent).toBe("min 1 · median 2 · max 3 (over 2 runs)");
    expect(screen.getByTestId("strip-text-checkPassRate").textContent).toBe("min 100% · median 100% · max 100% (over 1 run)");
    const svg = within(screen.getByTestId("strip-tokens")).getByRole("img");
    expect(svg.getAttribute("aria-label")).toBe("Tokens (in + out): min 1,500 · median 2,000 · max 2,500 (over 2 runs)");
    // The running run adds no dot: two finished runs, two dots.
    expect(svg.querySelectorAll("circle")).toHaveLength(2);
    // A measure with no figure says so.
    cleanup();
    await open({ distributions: { ...results().distributions, retries: dist(null, null, null, 0) } });
    expect(screen.getByTestId("strip-text-retries").textContent).toBe("no figures");
  });

  it("lists each run with its Checks column: a pass rate, checking…, or not checked", async () => {
    await open({
      runs: [
        run(1),
        run(2, { checkPassRate: null, checkState: "not checked" }),
        run(3, { checkPassRate: null, checkState: "checking" }),
      ],
      finished: 3,
      running: 0,
    });
    expect(screen.getByTestId("eval-checks-1").textContent).toBe("100% pass");
    expect(screen.getByTestId("eval-checks-2").textContent).toBe("not checked");
    expect(screen.getByTestId("eval-checks-3").textContent).toBe("checking…");
    expect(screen.getByTestId("eval-run-1").textContent).toContain("Eval run finished");
    // A run still going shows no figure at all, never 0.
    cleanup();
    await open();
    expect(screen.getByTestId("eval-run-3").textContent).toContain("Running");
    expect(screen.getByTestId("eval-checks-3").textContent).toBe("—");
    expect(screen.getByTestId("eval-run-3").textContent).not.toContain("0");
  });

  it("confirms the clean up when a run is not checked, and lists one line per worktree", async () => {
    evalCleanUp.mockResolvedValue([{ index: 1, runDir: "/d/runs/r-1", lines: ["PR1: worktree removed; branch dcv2/x kept (not merged)"] }]);
    await open();
    fireEvent.click(screen.getByRole("button", { name: "Clean up" }));
    const dialog = await screen.findByRole("dialog", { name: "Clean up this group?" });
    expect(dialog.textContent).toContain("Eval run 2 of 3 is not checked yet");
    expect(dialog.textContent).toContain('check pass rate will stay "not checked"');
    expect(evalCleanUp).not.toHaveBeenCalled();
    fireEvent.click(within(dialog).getByRole("button", { name: "Clean up anyway" }));
    await waitFor(() => expect(evalCleanUp).toHaveBeenCalledWith("b-1"));
    expect((await screen.findByTestId("eval-clean-lines")).textContent).toContain("Run 1: PR1: worktree removed");
  });

  it("cleans up at once when every finished run is checked", async () => {
    evalCleanUp.mockResolvedValue([]);
    await open({ runs: [run(1), run(2)], finished: 2, running: 0 });
    fireEvent.click(screen.getByRole("button", { name: "Clean up" }));
    await waitFor(() => expect(evalCleanUp).toHaveBeenCalledWith("b-1"));
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("deletes a kept branch only after a confirmation that names the branch and the commit count", async () => {
    evalDeleteBranch.mockResolvedValue("deleted");
    const kept = { prId: "PR1", branch: "dcv2/r-1-pr1", repoPath: "/w/app", unmergedCommits: 3 };
    await open({ runs: [run(1, { keptBranches: [kept] }), run(2)], finished: 2, running: 0 });
    const item = screen.getByTestId("kept-1-PR1");
    expect(item.textContent).toContain("dcv2/r-1-pr1");
    expect(item.textContent).toContain("3 unmerged commits");
    fireEvent.click(within(item).getByRole("button", { name: "Delete…" }));
    const dialog = await screen.findByRole("dialog", { name: "Delete this branch?" });
    expect(dialog.textContent).toContain("dcv2/r-1-pr1");
    expect(dialog.textContent).toContain("3 unmerged commits");
    expect(evalDeleteBranch).not.toHaveBeenCalled();
    // Cancel is focused first and deletes nothing.
    expect(document.activeElement?.textContent).toBe("Cancel");
    fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(evalDeleteBranch).not.toHaveBeenCalled();
    fireEvent.click(within(screen.getByTestId("kept-1-PR1")).getByRole("button", { name: "Delete…" }));
    fireEvent.click(within(await screen.findByRole("dialog", { name: "Delete this branch?" })).getByRole("button", { name: "Delete branch" }));
    await waitFor(() => expect(evalDeleteBranch).toHaveBeenCalledWith("/d/runs/r-1", "PR1"));
  });

  it("says when no branch is kept, shows a read error, and opens a run's Trace", async () => {
    const onOpenTrace = vi.fn();
    await open({}, { onOpenTrace });
    expect(screen.getByText("No branch is kept.")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Eval run 1 of 3" }));
    expect(onOpenTrace).toHaveBeenCalledWith("/d/runs/r-1");
    cleanup();
    flightEvalResults.mockReset();
    flightEvalResults.mockRejectedValue("No such eval group.");
    render(<EvalResults group="b-x" onClose={noop} />);
    expect((await screen.findByTestId("eval-results-error")).textContent).toBe("No such eval group.");
  });
});
