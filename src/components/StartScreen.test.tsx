// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { RunListItem } from "../run/api";

const day = (n: number) => new Date(Date.now() - n * 86_400_000).toISOString();
const run = (n: number, over: Partial<RunListItem> = {}): RunListItem => ({
  runId: `r-${n}`,
  runDir: `/d/runs/r-${n}`,
  folder: "/w/app",
  createdAt: day(n),
  stage: "done",
  updatedAt: null,
  title: `Run ${n}`,
  hasReport: true,
  ...over,
});
// Newest first: 10 runs, the 2nd unfinished.
const runs = Array.from({ length: 10 }, (_, n) => run(n + 1, n === 1 ? { stage: "building", hasReport: false } : {}));

const readRunFile = vi.fn(async (_dir: string, _path: string) => "# the report");
const revealItemInDir = vi.fn(async (_path: string) => {});
vi.mock("../run/api", () => ({
  runOverview: vi.fn(async () => ({ pluginDir: "/p", pluginVersion: null, roles: [], runs })),
  detectFolderRepos: vi.fn(async () => ({ kind: "none", repos: [] })),
  startRun: vi.fn(),
  resumeRun: vi.fn(),
  readRunFile: (dir: string, path: string) => readRunFile(dir, path),
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn(async () => null) }));
vi.mock("@tauri-apps/plugin-opener", () => ({
  revealItemInDir: (path: string) => revealItemInDir(path),
  openUrl: vi.fn(),
}));
vi.mock("./WorktreeSetup", () => ({ WorktreeSetup: () => null }));
vi.mock("./MemoryPanel", () => ({ MemoryPanel: () => null }));
vi.mock("../clickup/ClickUpIntake", () => ({ ClickUpIntake: () => null }));

const { StartScreen } = await import("./StartScreen");

describe("StartScreen earlier runs", () => {
  afterEach(cleanup);

  it("lists the 8 newest with a relative date, and Show all opens the Runs view", async () => {
    const onShowAllRuns = vi.fn();
    render(
      <StartScreen windowId="main" initialFolder="" onStarted={() => {}} onPlainTab={() => {}} onShowAllRuns={onShowAllRuns} />,
    );
    const list = await screen.findByRole("region", { name: "Earlier runs" });
    expect(within(list).getAllByRole("listitem")).toHaveLength(8);
    expect(within(list).getByText("Run 1").parentElement!.textContent).toContain("yesterday");
    expect(within(list).getByText("Run 4").parentElement!.textContent).toContain("4 days ago");
    expect(within(list).queryByText("Run 9")).toBeNull();
    fireEvent.click(within(list).getByRole("button", { name: "Show all (10)" }));
    expect(onShowAllRuns).toHaveBeenCalledOnce();
  });

  it("offers Report and Open folder on done runs and Resume on the others", async () => {
    render(
      <StartScreen windowId="main" initialFolder="" onStarted={() => {}} onPlainTab={() => {}} onShowAllRuns={() => {}} />,
    );
    const list = await screen.findByRole("region", { name: "Earlier runs" });
    const done = within(within(list).getByText("Run 1").closest("li")!);
    const open = within(within(list).getByText("Run 2").closest("li")!);
    expect(done.getByText("Report")).toBeTruthy();
    expect(done.queryByText("Resume")).toBeNull();
    expect(open.getByText("Resume")).toBeTruthy();
    expect(open.queryByText("Report")).toBeNull();
    expect(open.queryByText("Open folder")).toBeNull();
    fireEvent.click(done.getByText("Open folder"));
    expect(revealItemInDir).toHaveBeenCalledWith("/d/runs/r-1");
    fireEvent.click(done.getByText("Report"));
    expect(await screen.findByText("# the report")).toBeTruthy();
    expect(readRunFile).toHaveBeenCalledWith("/d/runs/r-1", "report.md");
  });
});
