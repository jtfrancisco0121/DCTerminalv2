// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { RerunInfo, RerunRecord } from "../flight/types";

const flightReplayCheckInfo = vi.fn();
const flightReplayCheck = vi.fn();
const flightReplayCheckStop = vi.fn();
const flightReplayChecks = vi.fn();
const flightReplayCheckLog = vi.fn();
vi.mock("../flight/api", () => ({
  flightReplayCheckInfo: (...a: unknown[]) => flightReplayCheckInfo(...a),
  flightReplayCheck: (...a: unknown[]) => flightReplayCheck(...a),
  flightReplayCheckStop: (...a: unknown[]) => flightReplayCheckStop(...a),
  flightReplayChecks: (...a: unknown[]) => flightReplayChecks(...a),
  flightReplayCheckLog: (...a: unknown[]) => flightReplayCheckLog(...a),
}));

const { RerunCheck } = await import("./RerunCheck");

const info = (over: Partial<RerunInfo> = {}): RerunInfo => ({
  reason: null,
  command: "npm run check",
  setup: "npm ci",
  cwd: "server",
  commit: "3f2a9c1e0000000000000000000000000000abcd",
  clean: true,
  recorded: { result: "fail", exitCode: 1, finishedAt: "2026-11-05T13:00:00Z", clean: true },
  running: false,
  ...over,
});

const record = (over: Partial<RerunRecord> = {}): RerunRecord => ({
  schema: 1,
  runId: "r-1",
  pr: "PR1",
  n: 1,
  repo: "/ws/toy",
  worktree: "/ws/toy-worktrees/dcv2-replay-r-1-pr1-1",
  commit: "3f2a9c1e0000000000000000000000000000abcd",
  command: "npm run check",
  setup: "npm ci",
  cwd: "server",
  recorded: { result: "fail", exitCode: 1, finishedAt: "2026-11-05T13:00:00Z", clean: true },
  state: "preparing",
  exitCode: null,
  startedAt: "2026-11-05T13:05:00Z",
  finishedAt: null,
  detail: null,
  log: "PR1-1.log",
  ...over,
});

beforeEach(() => {
  flightReplayCheckInfo.mockReset().mockResolvedValue(info());
  flightReplayCheck.mockReset().mockResolvedValue(record());
  flightReplayCheckStop.mockReset().mockResolvedValue(undefined);
  flightReplayChecks.mockReset().mockResolvedValue([]);
  flightReplayCheckLog.mockReset().mockResolvedValue("");
});
afterEach(cleanup);

const open = async (props: { appCheck?: boolean } = {}) => {
  render(<RerunCheck runDir="/runs/r-1" pr="PR1" appCheck={props.appCheck ?? true} pollMs={20} />);
  await screen.findByRole("button", { name: "Re-run check" });
};

describe("RerunCheck", () => {
  it("shows why it is disabled as visible text", async () => {
    await open({ appCheck: false });
    expect((screen.getByRole("button", { name: "Re-run check" }) as HTMLButtonElement).disabled).toBe(true);
    expect(screen.getByTestId("rerun-reason").textContent).toBe("Only the app's own check can be re-run.");
    cleanup();
    for (const reason of [
      "Commit 3f2a9c1 is not in this repo (the branch may have been cleaned up).",
      "The recorded check has no command.",
      "a re-run is already running",
    ]) {
      flightReplayCheckInfo.mockResolvedValue(info({ reason }));
      render(<RerunCheck runDir="/runs/r-1" pr="PR1" appCheck pollMs={20} />);
      await waitFor(() => expect(screen.getByTestId("rerun-reason").textContent).toBe(reason));
      expect((screen.getByRole("button", { name: "Re-run check" }) as HTMLButtonElement).disabled).toBe(true);
      cleanup();
    }
  });

  it("needs two clicks: the confirmation names the setup, command, folder and commit, then Re-run starts it", async () => {
    await open();
    expect(flightReplayCheck).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Re-run check" }));
    const dialog = await screen.findByRole("dialog");
    expect(flightReplayCheck).not.toHaveBeenCalled();
    const text = within(dialog).getByTestId("rerun-confirm").textContent ?? "";
    expect(text).toContain("npm ci");
    expect(text).toContain("npm run check");
    expect(text).toContain("server");
    expect(text).toContain("3f2a9c1");
    expect(within(dialog).queryByText(/uncommitted changes/)).toBeNull();
    fireEvent.click(within(dialog).getByRole("button", { name: "Re-run" }));
    await waitFor(() => expect(flightReplayCheck).toHaveBeenCalledWith("/runs/r-1", "PR1"));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });

  it("leaves out the folder at the root and adds the note when the recorded check was not clean", async () => {
    flightReplayCheckInfo.mockResolvedValue(info({ cwd: "", setup: null, clean: false }));
    await open();
    fireEvent.click(screen.getByRole("button", { name: "Re-run check" }));
    const dialog = await screen.findByRole("dialog");
    const text = dialog.textContent ?? "";
    expect(text).not.toContain("Folder:");
    expect(text).not.toContain("Setup:");
    expect(text).toContain("uncommitted changes");
    fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
    expect(flightReplayCheck).not.toHaveBeenCalled();
  });

  it("shows re-run: pass beside recorded: fail, with the log's last 40 lines", async () => {
    const lines = Array.from({ length: 60 }, (_, i) => `line ${i + 1}`).join("\n");
    flightReplayChecks.mockResolvedValue([record({ state: "pass", exitCode: 0, finishedAt: "2026-11-05T13:09:00Z" })]);
    flightReplayCheckLog.mockResolvedValue(lines);
    await open();
    const recorded = await screen.findByTestId("rerun-recorded");
    const latest = await screen.findByTestId("rerun-latest");
    expect(recorded.textContent).toContain("recorded: fail");
    expect(latest.textContent).toContain("re-run: pass");
    await waitFor(() => expect(latest.textContent).toContain("line 60"));
    expect(latest.textContent).not.toContain("line 20\n");
    expect(latest.textContent).toContain("line 21");
    fireEvent.click(within(latest).getByRole("button", { name: "Open full log" }));
    expect((await screen.findByTestId("rerun-full-log")).textContent).toContain("line 1\n");
  });

  it("shows the running step, a polite live region and Stop, and stopping calls the backend", async () => {
    flightReplayChecks.mockResolvedValue([record({ state: "setup" })]);
    await open();
    expect((await screen.findByTestId("rerun-latest")).textContent).toContain("re-run: setting up");
    expect(screen.getByTestId("rerun-live").getAttribute("aria-live")).toBe("polite");
    await waitFor(() => expect(screen.getByTestId("rerun-live").textContent).toBe("Re-run: setting up"));
    fireEvent.click(screen.getByRole("button", { name: "Stop" }));
    await waitFor(() => expect(flightReplayCheckStop).toHaveBeenCalled());
    flightReplayChecks.mockResolvedValue([record({ state: "stopped" })]);
    await waitFor(() => expect(screen.getByTestId("rerun-latest").textContent).toContain("re-run: stopped"));
    expect(screen.queryByRole("button", { name: "Stop" })).toBeNull();
    await waitFor(() => expect(screen.getByTestId("rerun-live").textContent).toBe("Re-run finished: stopped"));
  });

  it("lists the earlier re-runs by number", async () => {
    flightReplayChecks.mockResolvedValue([record({ n: 3, state: "pass" }), record({ n: 2, state: "fail", exitCode: 1 }), record({ n: 1, state: "interrupted" })]);
    await open();
    const list = await screen.findByRole("list", { name: "Earlier re-runs" });
    expect(list.textContent).toContain("Re-run 2: fail");
    expect(list.textContent).toContain("Re-run 1: interrupted (the app quit)");
  });

  it("shows a start failure and keeps the dialog", async () => {
    flightReplayCheck.mockRejectedValue("a re-run is already running");
    await open();
    fireEvent.click(screen.getByRole("button", { name: "Re-run check" }));
    const dialog = await screen.findByRole("dialog");
    fireEvent.click(within(dialog).getByRole("button", { name: "Re-run" }));
    await waitFor(() => expect(within(dialog).getByRole("alert").textContent).toBe("a re-run is already running"));
  });
});
