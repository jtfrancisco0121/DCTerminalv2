// @vitest-environment jsdom
import { cleanup, render, screen, within } from "@testing-library/react";
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

  it("draws the stage strip, labelling wide bands and only titling narrow ones", () => {
    const withStages: Timeline = {
      ...timeline,
      bands: [
        ...timeline.bands,
        { kind: "stage", name: "building", startMs: 0, endMs: 540000 },
        { kind: "stage", name: "fixing", startMs: 540000, endMs: 570000 },
      ],
    };
    render(<FleetTimelineView timeline={withStages} />);
    const strip = within(screen.getByTestId("fleet-stages"));
    expect(strip.getByText("Building")).toBeTruthy();
    expect(strip.queryByText("fixing")).toBeNull();
    expect(screen.getByTitle("fixing")).toBeTruthy();
  });

  it("has no strip without stage bands, and an axis with two local times", () => {
    render(<FleetTimelineView timeline={timeline} />);
    expect(screen.queryByTestId("fleet-stages")).toBeNull();
    const axis = screen.getByTestId("fleet-axis");
    expect(axis.querySelectorAll("[title]")).toHaveLength(2);
    expect(axis.textContent).toMatch(/\d\d:\d\d\d\d:\d\d/);
    cleanup();
    render(<FleetTimelineView timeline={{ ...timeline, bands: [], startedMs: null, endedMs: null, lanes: [{ ...timeline.lanes[0]!, startMs: null, finishMs: null }] }} />);
    expect(screen.queryByTestId("fleet-axis")).toBeNull();
  });

  it("shows each lane's duration and cost", () => {
    render(<FleetTimelineView timeline={timeline} />);
    const lanes = screen.getAllByRole("listitem");
    expect(lanes[0]!.querySelector(".fleet-lane-total")?.textContent).toBe("10 min · $0.40");
    expect(lanes[1]!.querySelector(".fleet-lane-total")?.textContent).toBe("3 min · $0.10");
    expect(lanes[2]!.querySelector(".fleet-lane-total")?.textContent).toBe("7 min · cost unknown so far");
  });

  it("says so when nothing was recorded", () => {
    render(<FleetTimelineView timeline={{ ...timeline, lanes: [] }} />);
    expect(screen.getByText(/No agents were recorded/)).toBeTruthy();
  });
});
