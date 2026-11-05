import { beforeEach, describe, expect, it, vi } from "vitest";
import type { OpenPlan } from "./live";
import type { TabSummary } from "../bridge";
import { clearUiErrors, uiErrors } from "../uiErrors";

const live = vi.hoisted(() => ({
  running: new Set<string>(),
  opening: new Set<string>(),
  open: vi.fn(),
  drop: vi.fn(),
}));
vi.mock("./live", () => ({
  ptyRunning: (id: string) => live.running.has(id),
  isPtyOpening: (id: string) => live.opening.has(id),
  openLivePty: (id: string, plan: OpenPlan<unknown>) => live.open(id, plan),
  dropLivePty: (id: string) => live.drop(id),
}));
const park = vi.hoisted(() => ({ ensure: vi.fn() }));
vi.mock("./park", () => ({
  ensureParkedTerminal: (...args: unknown[]) => park.ensure(...args),
  measuredTerminalSize: () => ({ cols: 80, rows: 24 }),
}));
vi.mock("../bridge", () => ({ terminalResumeId: vi.fn(async () => null) }));

const { openInBackground } = await import("./background");

const tab: TabSummary = {
  id: "tab_run",
  label: "Eagle-Eye · ws",
  customLabel: true,
  cwd: "/w/ws",
  kind: "claude",
  folderStatus: "ok",
  hasSession: false,
  pendingStart: true,
};

beforeEach(() => {
  live.running.clear();
  live.opening.clear();
  live.open.mockReset();
  live.drop.mockReset();
  park.ensure.mockReset();
  clearUiErrors();
});

describe("opening a tab's terminal in the background", () => {
  it("parks a terminal and opens once with the measured size, a fresh start", async () => {
    openInBackground(tab, 14, false);
    expect(park.ensure).toHaveBeenCalledWith("tab_run", 14);
    expect(live.open).toHaveBeenCalledTimes(1);
    const [id, plan] = live.open.mock.calls[0] as [string, OpenPlan<boolean>];
    expect(id).toBe("tab_run");
    expect(plan.ifIdle).toBe(true);
    const resume = await plan.before!();
    expect(plan.start(resume)).toEqual({ cols: 80, rows: 24, resume: false });
  });

  it("does nothing for a running or opening tab, a missing folder or a removed account", () => {
    live.running.add("tab_run");
    openInBackground(tab, 14, false);
    live.running.clear();
    live.opening.add("tab_run");
    openInBackground(tab, 14, false);
    live.opening.clear();
    openInBackground({ ...tab, folderStatus: "missing" }, 14, false);
    openInBackground(tab, 14, true);
    expect(live.open).not.toHaveBeenCalled();
    expect(park.ensure).not.toHaveBeenCalled();
  });

  it("reports a failed open once and drops the session so the next try starts again", () => {
    openInBackground(tab, 14, false);
    const plan = (live.open.mock.calls[0] as [string, OpenPlan<boolean>])[1];
    plan.onError!("spawn failed");
    plan.onError!("spawn failed");
    expect(live.drop).toHaveBeenCalledWith("tab_run");
    expect(uiErrors().filter((e) => e.source === "Start run tab")).toHaveLength(1);
  });
});
