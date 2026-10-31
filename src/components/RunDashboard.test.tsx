// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ActiveRun } from "../bridge";

const focusTab = vi.fn(async (_id: string) => null);
vi.mock("../bridge", () => ({ focusTab: (id: string) => focusTab(id) }));

const { RunDashboard, CLOCK_MS, minutesSince } = await import("./RunDashboard");

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
  prsTotal: 2,
  prs: [
    { id: "PR1", state: "opened", appCheck: null, ci: null, prState: null },
    { id: "PR2", state: "building", appCheck: null, ci: null, prState: null },
  ],
  ...over,
});

describe("RunDashboard", () => {
  beforeEach(() => {
    vi.useFakeTimers({ now: new Date("2026-11-01T12:00:00Z") });
    focusTab.mockClear();
  });
  afterEach(() => {
    cleanup();
    vi.useRealTimers();
  });

  it("shows a card per unfinished run tab with steps and one pill per PR, and leaves finished runs out", () => {
    render(<RunDashboard runs={[run("a"), run("b", { windowLabel: "win-2" }), run("c", { stage: "done" }), run("d", { stage: "failed" })]} onClose={() => {}} />);
    expect(screen.getAllByTestId(/^card-/).map((el) => el.getAttribute("data-testid"))).toEqual(["card-a", "card-b"]);
    const a = within(screen.getByTestId("card-a"));
    expect(a.getByRole("list", { name: "Run steps" })).toBeTruthy();
    expect(a.getByText("PR1: PR opened")).toBeTruthy();
    expect(a.getByText("PR2: building")).toBeTruthy();
  });

  it("says how long a run has waited and re-counts on the minute clock without any command", () => {
    const waiting = run("w", { stage: "plan-approval", waitingFor: "plan-approval", waitingSince: "2026-11-01T11:55:00Z" });
    render(<RunDashboard runs={[waiting]} onClose={() => {}} />);
    expect(screen.getByText(/waiting for you · 5 min/)).toBeTruthy();
    act(() => {
      vi.advanceTimersByTime(CLOCK_MS * 2);
    });
    expect(screen.getByText(/waiting for you · 7 min/)).toBeTruthy();
    expect(focusTab).not.toHaveBeenCalled();
  });

  it("waits without a start time say only waiting for you; stalled runs say no progress", () => {
    render(<RunDashboard runs={[run("w", { waitingFor: "plan-approval" }), run("s", { stalled: true })]} onClose={() => {}} />);
    expect(within(screen.getByTestId("card-w")).getByText("waiting for you")).toBeTruthy();
    expect(within(screen.getByTestId("card-s")).getByText("no progress")).toBeTruthy();
  });

  it("Go to tab focuses the tab and closes the page", () => {
    const onClose = vi.fn();
    render(<RunDashboard runs={[run("a")]} onClose={onClose} />);
    fireEvent.click(screen.getByText("Go to tab"));
    expect(focusTab).toHaveBeenCalledWith("a");
    expect(onClose).toHaveBeenCalled();
  });

  it("shows an empty state, and Close closes", () => {
    const onClose = vi.fn();
    render(<RunDashboard runs={[]} onClose={onClose} />);
    expect(screen.getByText("No run is in progress.")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Close" }));
    expect(onClose).toHaveBeenCalled();
  });

  it("minutesSince is null for a missing or unreadable time and never negative", () => {
    const now = Date.parse("2026-11-01T12:00:00Z");
    expect(minutesSince(null, now)).toBeNull();
    expect(minutesSince("nope", now)).toBeNull();
    expect(minutesSince("2026-11-01T12:30:00Z", now)).toBe(0);
  });
});
