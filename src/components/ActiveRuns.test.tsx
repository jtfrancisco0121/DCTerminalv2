// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ActiveRun } from "../bridge";

const run = (id: string, over: Partial<ActiveRun> = {}): ActiveRun => ({
  windowLabel: "main",
  tabId: id,
  label: `Eagle-Eye · ${id}`,
  activity: "working",
  runId: `r-${id}`,
  runDir: `/d/runs/r-${id}`,
  stage: "building",
  waitingFor: null,
  stalled: false,
  prsDone: 1,
  prsTotal: 3,
  ...over,
});
let list: ActiveRun[] = [];
const activeRuns = vi.fn(async () => list);
const focusTab = vi.fn(async (_id: string) => null);
vi.mock("../bridge", () => ({
  activeRuns: () => activeRuns(),
  focusTab: (id: string) => focusTab(id),
}));

const { ActiveRuns, POLL_MS } = await import("./ActiveRuns");

describe("ActiveRuns", () => {
  beforeEach(() => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    activeRuns.mockClear();
  });
  afterEach(() => {
    cleanup();
    vi.useRealTimers();
  });

  it("says 1 PR for one PR and n PRs for more", async () => {
    list = [run("one", { prsDone: 0, prsTotal: 1 }), run("two", { prsDone: 1, prsTotal: 2 })];
    render(<ActiveRuns onClose={() => {}} onAllRuns={() => {}} />);
    expect((await screen.findByTestId("active-one")).textContent).toContain("0/1 PR");
    expect(screen.getByTestId("active-one").textContent).not.toContain("0/1 PRs");
    expect(screen.getByTestId("active-two").textContent).toContain("1/2 PRs");
  });

  it("words a finished eval run as finished, with no waiting flag", async () => {
    list = [run("e", { stage: "ship-approval", eval: { group: "b-1", index: 1, of: 2, source: "r-0" } })];
    render(<ActiveRuns onClose={() => {}} onAllRuns={() => {}} />);
    const item = await screen.findByTestId("active-e");
    expect(item.textContent).toContain("Eval run finished");
    expect(item.textContent).not.toContain("waiting for you");
    expect(item.textContent).not.toContain("Waiting for your go");
  });

  it("lists runs of every window with stage, waiting/stalled flags and PR progress; Go to tab focuses", async () => {
    list = [
      run("a"),
      run("b", { windowLabel: "win-2", waitingFor: "plan-approval", stage: "plan-approval", prsTotal: 0 }),
      run("c", { stalled: true }),
    ];
    const onClose = vi.fn();
    render(<ActiveRuns onClose={onClose} onAllRuns={() => {}} />);
    await act(async () => {});
    const a = within(screen.getByTestId("active-a"));
    expect(a.getByText(/Building · 1\/3 PRs/)).toBeTruthy();
    expect(a.getByRole("list", { name: "Run steps" }).querySelectorAll("li")).toHaveLength(6);
    expect(within(screen.getByTestId("active-b")).getByText("waiting for you")).toBeTruthy();
    expect(within(screen.getByTestId("active-c")).getByText("no progress")).toBeTruthy();
    fireEvent.click(within(screen.getByTestId("active-b")).getByText("Go to tab"));
    expect(focusTab).toHaveBeenCalledWith("b");
    expect(onClose).toHaveBeenCalled();
  });

  it("shows three steps for an epic-plan run", async () => {
    list = [run("e", { runMode: "epic-plan", stage: "planning", prsTotal: 0 })];
    render(<ActiveRuns onClose={() => {}} onAllRuns={() => {}} />);
    await act(async () => {});
    expect(within(screen.getByTestId("active-e")).getByRole("list", { name: "Run steps" }).querySelectorAll("li")).toHaveLength(3);
  });

  it("polls at most every 3 s while open, closes on Escape, and the footer opens All runs", async () => {
    list = [];
    const onClose = vi.fn();
    const onAllRuns = vi.fn();
    render(<ActiveRuns onClose={onClose} onAllRuns={onAllRuns} />);
    await act(async () => {});
    expect(screen.getByText("No run is in progress.")).toBeTruthy();
    expect(activeRuns).toHaveBeenCalledTimes(1);
    await act(async () => {
      vi.advanceTimersByTime(POLL_MS - 100);
    });
    expect(activeRuns).toHaveBeenCalledTimes(1);
    await act(async () => {
      vi.advanceTimersByTime(200);
    });
    expect(activeRuns).toHaveBeenCalledTimes(2);
    fireEvent.keyDown(window, { key: "Escape" });
    expect(onClose).toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "All runs" }));
    expect(onAllRuns).toHaveBeenCalledOnce();
  });
  it("Open dashboard calls back and closes the popover; absent without the callback", async () => {
    list = [run("a")];
    const onDashboard = vi.fn();
    const onClose = vi.fn();
    const { unmount } = render(<ActiveRuns onClose={onClose} onAllRuns={() => {}} onDashboard={onDashboard} />);
    await act(async () => {});
    fireEvent.click(screen.getByText("Open dashboard"));
    expect(onDashboard).toHaveBeenCalled();
    expect(onClose).toHaveBeenCalled();
    unmount();
    render(<ActiveRuns onClose={onClose} onAllRuns={() => {}} />);
    await act(async () => {});
    expect(screen.queryByText("Open dashboard")).toBeNull();
  });
});
