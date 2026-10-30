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
    expect(within(screen.getByTestId("active-b")).getByText("waiting for you")).toBeTruthy();
    expect(within(screen.getByTestId("active-c")).getByText("no progress")).toBeTruthy();
    fireEvent.click(within(screen.getByTestId("active-b")).getByText("Go to tab"));
    expect(focusTab).toHaveBeenCalledWith("b");
    expect(onClose).toHaveBeenCalled();
  });

  it("shows one line per epic only for 2+ groups", async () => {
    list = [
      run("m", { epics: [{ id: "e1", title: "Billing", done: 1, total: 2 }, { id: null, title: null, done: 0, total: 1 }] }),
      run("s", { epics: [{ id: "e1", title: "Billing", done: 1, total: 2 }] }),
      run("o"),
    ];
    render(<ActiveRuns onClose={() => {}} onAllRuns={() => {}} />);
    await act(async () => {});
    const lines = within(screen.getByTestId("active-epics-m")).getAllByRole("listitem").map((li) => li.textContent);
    expect(lines).toEqual(["Billing · 1/2 PRs", "No epic · 0/1 PRs"]);
    expect(screen.queryByTestId("active-epics-s")).toBeNull();
    expect(screen.queryByTestId("active-epics-o")).toBeNull();
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
});
