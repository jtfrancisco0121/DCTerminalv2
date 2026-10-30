// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const startRun = vi.hoisted(() => vi.fn());
const dispatchOverview = vi.hoisted(() => vi.fn());
const dispatchEnqueue = vi.hoisted(() => vi.fn());
vi.mock("../run/api", async () => ({
  duplicateOf: () => null,
  runOverview: vi.fn(async () => ({ pluginDir: "/p", pluginVersion: null, roles: [], runs: [] })),
  detectFolderRepos: vi.fn(async () => ({ kind: "repo", repos: [{ name: "api", path: "/w/api", check: null }] })),
  startRun,
  resumeRun: vi.fn(),
  readRunFile: vi.fn(),
}));
vi.mock("../run/dispatch", () => ({ dispatchOverview, dispatchEnqueue }));
vi.mock("../bridge", () => ({
  recentFolders: vi.fn(async () => []),
  getSettings: vi.fn(async () => ({ run: { maxImplementers: 3, stopHooks: true } })),
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn(async () => null) }));
vi.mock("@tauri-apps/plugin-opener", () => ({ revealItemInDir: vi.fn(), openUrl: vi.fn() }));
vi.mock("./WorktreeSetup", () => ({ WorktreeSetup: () => null }));
vi.mock("./MemoryPanel", () => ({ MemoryPanel: () => null }));
vi.mock("../clickup/ClickUpIntake", () => ({ ClickUpIntake: () => null }));

const { StartScreen } = await import("./StartScreen");

beforeEach(() => {
  startRun.mockReset();
  startRun.mockResolvedValue({ tab: {}, runDir: "/d/runs/new" });
  dispatchEnqueue.mockReset();
  dispatchEnqueue.mockResolvedValue("q-1");
  dispatchOverview.mockReset();
});
afterEach(cleanup);

const start = async () => {
  render(<StartScreen windowId="main" initialFolder="/w/api" onStarted={() => {}} onPlainTab={() => {}} />);
  fireEvent.change(screen.getByPlaceholderText(/Describe the feature/), { target: { value: "Do it" } });
  const button = await screen.findByRole("button", { name: "Start Eagle-Eye" });
  await vi.waitFor(() => expect((button as HTMLButtonElement).disabled).toBe(false));
  fireEvent.click(button);
};

describe("a manual start over the cap", () => {
  it("asks first, listing the slot holders, and starts nothing until a choice", async () => {
    dispatchOverview.mockResolvedValue({ atCap: true, cap: 2, queue: [], usageHoldPercent: null, holders: [{ runDir: "/d/runs/a", runId: "r-a", stage: "building", why: "tab open" }] });
    await start();
    expect(await screen.findByText("The run limit is reached")).toBeTruthy();
    expect(screen.getByTestId("cap-holders").textContent).toContain("r-a");
    expect(startRun).not.toHaveBeenCalled();
    expect(dispatchEnqueue).not.toHaveBeenCalled();
  });

  it("Queue it stores the start with its choices and starts no run", async () => {
    dispatchOverview.mockResolvedValue({ atCap: true, cap: 1, queue: [], usageHoldPercent: null, holders: [] });
    await start();
    fireEvent.click(await screen.findByRole("button", { name: "Queue it" }));
    await vi.waitFor(() => expect(dispatchEnqueue).toHaveBeenCalledOnce());
    const params = dispatchEnqueue.mock.calls[0][0];
    expect(params).toMatchObject({ folder: "/w/api", request: "Do it", role: "eagle-eye" });
    expect(startRun).not.toHaveBeenCalled();
  });

  it("Start now goes through the normal start", async () => {
    dispatchOverview.mockResolvedValue({ atCap: true, cap: 1, queue: [], usageHoldPercent: null, holders: [] });
    await start();
    fireEvent.click(await screen.findByRole("button", { name: "Start now" }));
    await vi.waitFor(() => expect(startRun).toHaveBeenCalledOnce());
    expect(dispatchEnqueue).not.toHaveBeenCalled();
  });

  it("below the cap it starts as before with no question", async () => {
    dispatchOverview.mockResolvedValue({ atCap: false, cap: 2, queue: [], usageHoldPercent: null, holders: [] });
    await start();
    await vi.waitFor(() => expect(startRun).toHaveBeenCalledOnce());
    expect(screen.queryByText("The run limit is reached")).toBeNull();
  });
});
