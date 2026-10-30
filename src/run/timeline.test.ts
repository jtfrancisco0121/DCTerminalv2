import { describe, expect, it } from "vitest";
import { barName, layoutTimeline, minutesText, type Lane, type Timeline } from "./timeline";

const lane = (over: Partial<Lane>): Lane => ({ id: "x", label: "implementer", description: null, state: "completed", startMs: 0, finishMs: 1000, costUsd: 0.5, ...over });
const base = (lanes: Lane[], over: Partial<Timeline> = {}): Timeline => ({
  lanes,
  bands: [],
  workMinutes: 8,
  waitMinutes: 3.5,
  costUsd: 1.25,
  startedMs: 0,
  endedMs: 1000,
  ...over,
});

describe("layoutTimeline", () => {
  it("puts the orchestrator first, then lanes by start, as percentages of the span", () => {
    const t = base([
      lane({ id: "b", label: "reviewer", startMs: 500, finishMs: 1000 }),
      lane({ id: "o", label: "orchestrator", startMs: 0, finishMs: 1000 }),
      lane({ id: "a", label: "planner", startMs: 100, finishMs: 300 }),
    ]);
    const { bars } = layoutTimeline(t);
    expect(bars.map((b) => b.lane.label)).toEqual(["orchestrator", "planner", "reviewer"]);
    expect(bars[1]!.left).toBeCloseTo(10);
    expect(bars[1]!.width).toBeCloseTo(20);
  });

  it("draws a lane with no finish open to the end, and keeps an unknown agent as its own lane", () => {
    const { bars } = layoutTimeline(base([lane({ label: "custom:mystery", state: "unfinished", startMs: 400, finishMs: null })]));
    expect(bars[0]!.open).toBe(true);
    expect(bars[0]!.left + bars[0]!.width).toBeCloseTo(100);
    expect(bars[0]!.lane.label).toBe("custom:mystery");
  });

  it("places approval waits as their own shaded bands", () => {
    const t = base([lane({})], { bands: [{ kind: "wait", name: "waiting for plan approval", startMs: 200, endMs: 600 }, { kind: "stage", name: "building", startMs: 0, endMs: 1000 }] });
    const { waits } = layoutTimeline(t);
    expect(waits).toHaveLength(1);
    expect(waits[0]!.left).toBeCloseTo(20);
    expect(waits[0]!.width).toBeCloseTo(40);
  });

  it("states work time and waiting time apart in words", () => {
    expect(minutesText(8)).toBe("8 min");
    expect(minutesText(0.4)).toBe("under a minute");
    expect(minutesText(75)).toBe("1 h 15 min");
  });

  it("names a bar with its label, state word, start and cost", () => {
    const name = barName(lane({ label: "implementer", description: "Build PR1", state: "unfinished", startMs: Date.UTC(2026, 9, 30, 10, 3), finishMs: null, costUsd: null }));
    expect(name).toContain("implementer, Build PR1");
    expect(name).toContain("unfinished (no finish recorded)");
    expect(name).toContain("10:03 UTC");
    expect(name).toContain("cost unknown");
  });
});
