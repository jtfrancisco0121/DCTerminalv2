// @vitest-environment jsdom
import { act, cleanup, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { LogLine } from "./api";
import { AWAY_GAP_MS, linesWhileAway, readMarker, useAwayNotice, writeMarker } from "./away";

const NOW = Date.parse("2026-10-27T14:30:00Z");
const line = (minutesAgo: number, text = `step ${minutesAgo}`): LogLine => ({
  time: new Date(NOW - minutesAgo * 60_000).toISOString(),
  stage: "building",
  text,
});

describe("linesWhileAway", () => {
  it("lists only newer lines, and only after a gap of 10 minutes", () => {
    const timeline = [line(50), line(30), line(5)];
    const since = NOW - 40 * 60_000;
    expect(linesWhileAway(timeline, since, NOW).map((l) => l.text)).toEqual(["step 30", "step 5"]);
    expect(linesWhileAway(timeline, NOW - AWAY_GAP_MS + 1_000, NOW)).toEqual([]);
    expect(linesWhileAway(timeline, NOW + 1, NOW + AWAY_GAP_MS)).toEqual([]);
    expect(linesWhileAway([{ time: null, stage: null, text: "no time" }], since, NOW)).toEqual([]);
    expect(linesWhileAway([], since, NOW)).toEqual([]);
  });
});

describe("useAwayNotice", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(NOW);
    window.localStorage.clear();
  });
  afterEach(() => {
    cleanup();
    vi.useRealTimers();
    vi.restoreAllMocks();
  });

  const render = (timeline: LogLine[], ready = true) =>
    renderHook((props: { timeline: LogLine[]; ready: boolean }) => useAwayNotice("main", "/runs/r-1", props.timeline, props.ready), {
      initialProps: { timeline, ready },
    });

  it("shows the lines newer than the marker after a gap of 10 minutes or more", () => {
    writeMarker("main", "/runs/r-1", NOW - 60 * 60_000);
    const { result } = render([line(90), line(30), line(5)]);
    expect(result.current?.lines.map((l) => l.text)).toEqual(["step 30", "step 5"]);
  });

  it("shows nothing after a short gap, with no new lines, with no marker yet, or before the run is read", () => {
    writeMarker("main", "/runs/r-1", NOW - 5 * 60_000);
    expect(render([line(1)]).result.current).toBeNull();
    writeMarker("main", "/runs/r-1", NOW - 60 * 60_000);
    expect(render([line(90)]).result.current).toBeNull();
    window.localStorage.clear();
    expect(render([line(5)]).result.current).toBeNull();
    writeMarker("main", "/runs/r-1", NOW - 60 * 60_000);
    expect(render([line(5)], false).result.current).toBeNull();
  });

  it("Dismiss hides it and records the view", () => {
    writeMarker("main", "/runs/r-1", NOW - 60 * 60_000);
    const { result } = render([line(30)]);
    expect(result.current).not.toBeNull();
    // While the notice is open the marker stays: new timeline lines do not advance it.
    expect(readMarker("main", "/runs/r-1")).toBe(NOW - 60 * 60_000);
    act(() => result.current!.dismiss());
    expect(result.current).toBeNull();
    expect(readMarker("main", "/runs/r-1")).toBe(NOW);
  });

  it("follows the timeline while in view, and shows again on a window focus after a gap", () => {
    const { result, rerender } = render([line(5)]);
    expect(result.current).toBeNull();
    expect(readMarker("main", "/runs/r-1")).toBe(NOW);
    act(() => {
      window.dispatchEvent(new Event("blur"));
    });
    act(() => {
      vi.setSystemTime(NOW + 30 * 60_000);
    });
    rerender({ timeline: [line(5), { time: new Date(NOW + 20 * 60_000).toISOString(), stage: null, text: "later" }], ready: true });
    act(() => {
      window.dispatchEvent(new Event("focus"));
    });
    expect(result.current?.lines.map((l) => l.text)).toEqual(["later"]);
  });

  it("shows nothing when localStorage is broken", () => {
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
      throw new Error("blocked");
    });
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new Error("blocked");
    });
    const { result } = render([line(30)]);
    expect(result.current).toBeNull();
    expect(readMarker("main", "/runs/r-1")).toBeNull();
  });
});
