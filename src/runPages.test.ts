import { describe, expect, it } from "vitest";
import type { RunListItem } from "./run/api";
import { compareDefaults, RUN_PAGES } from "./runPages";

const item = (runDir: string, folder: string, stage: string, outcome?: string): RunListItem =>
  ({ runId: runDir, runDir, folder, stage, record: outcome ? { outcome } : null }) as unknown as RunListItem;

describe("RUN_PAGES", () => {
  it("lists the pages in order, with Web QA only in the palette", () => {
    expect(RUN_PAGES.map((p) => p.id)).toEqual(["all-runs", "dashboard", "analytics", "trace-search", "compare", "web-qa"]);
    expect(RUN_PAGES.filter((p) => p.inPopover).map((p) => p.id)).toEqual(["all-runs", "dashboard", "analytics", "trace-search", "compare"]);
    expect(RUN_PAGES.find((p) => p.id === "web-qa")!.inPopover).toBe(false);
  });
});

describe("compareDefaults", () => {
  const runs = [
    item("/r/1", "/w/a", "building"),
    item("/r/2", "/w/b", "done", "done"),
    item("/r/3", "/w/a", "done", "done"),
    item("/r/4", "/w/a", "failed"),
    item("/r/5", "/w/a", "done", "done"),
  ];
  it("picks the two newest finished runs of the folder", () => {
    expect(compareDefaults(runs, "/w/a")).toEqual(["/r/3", "/r/4"]);
  });
  it("takes any folder when there is none, and nulls when there are fewer than two", () => {
    expect(compareDefaults(runs, null)).toEqual(["/r/2", "/r/3"]);
    expect(compareDefaults(runs, "/w/b")).toEqual(["/r/2", null]);
    expect(compareDefaults([], "/w/a")).toEqual([null, null]);
  });
});
