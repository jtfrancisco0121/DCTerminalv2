// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import type { Timeline } from "../run/timeline";
import { FleetTimelineView } from "./FleetTimeline";

afterEach(cleanup);

const timeline: Timeline = {
  lanes: [
    { id: "o", label: "orchestrator", description: null, state: "completed", startMs: 0, finishMs: 600000, costUsd: 0.4 },
    { id: "r", label: "plan-reviewer", description: "Review", state: "completed", startMs: 0, finishMs: 180000, costUsd: 0.1 },
    { id: "i", label: "implementer", description: "Build PR1", state: "unfinished", startMs: 190000, finishMs: null, costUsd: null },
  ],
  bands: [{ kind: "wait", name: "waiting for plan approval", startMs: 180000, endMs: 360000 }],
  workMinutes: 7,
  waitMinutes: 3,
  costUsd: 0.5,
  startedMs: 0,
  endedMs: 600000,
};

describe("FleetTimelineView", () => {
  it("shows work time and waiting time separately with the cost", () => {
    render(<FleetTimelineView timeline={timeline} />);
    const totals = screen.getByTestId("fleet-totals").textContent ?? "";
    expect(totals).toContain("Work 7 min");
    expect(totals).toContain("Waiting for you 3 min");
    expect(totals).toContain("$0.50");
  });

  it("draws one lane per agent with a focusable bar that names its state in text", () => {
    render(<FleetTimelineView timeline={timeline} />);
    const bars = screen.getAllByRole("img");
    expect(bars).toHaveLength(3);
    for (const bar of bars) expect(bar.getAttribute("tabindex")).toBe("0");
    const unfinished = screen.getByLabelText(/implementer, Build PR1: unfinished/);
    expect(unfinished).toBeTruthy();
    expect(screen.getByLabelText(/plan-reviewer, Review: completed/)).toBeTruthy();
  });

  it("says so when nothing was recorded", () => {
    render(<FleetTimelineView timeline={{ ...timeline, lanes: [] }} />);
    expect(screen.getByText(/No agents were recorded/)).toBeTruthy();
  });
});
