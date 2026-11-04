// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import fixture from "../../fixtures/run/run-view.json";
import type { Timeline } from "../run/timeline";

const timeline: Timeline = {
  lanes: [
    { id: "session-0", label: "orchestrator", description: null, state: "completed", startMs: 0, finishMs: 600000, costUsd: 0.4, session: 0, agentId: null },
    { id: "0-t1", label: "implementer", description: "PR1 build", state: "completed", startMs: 10, finishMs: 500000, costUsd: 0.1, session: 0, agentId: "a2f2a89910f21f43e" },
  ],
  bands: [],
  workMinutes: 9,
  waitMinutes: 0,
  costUsd: 0.5,
  startedMs: 0,
  endedMs: 600000,
};

vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}) }));
vi.mock("../run/timeline", async (original) => ({
  ...(await original<typeof import("../run/timeline")>()),
  runTimeline: vi.fn(async () => timeline),
}));
vi.mock("../run/api", async (original) => ({
  ...(await original<typeof import("../run/api")>()),
  readRun: vi.fn(async () => fixture),
  rerunChecks: vi.fn(async () => {}),
  prWorktreeChecks: vi.fn(async () => []),
}));

const { RunPanel } = await import("./RunPanel");

afterEach(cleanup);

describe("RunPanel trace entry points", () => {
  it("a lane click opens the trace on that lane's agent, for a run with no tab", async () => {
    const onOpenTrace = vi.fn();
    render(<RunPanel runDir="/d/runs/r-1" onOpenTrace={onOpenTrace} />);
    fireEvent.click(await screen.findByRole("button", { name: "Open trace for implementer" }));
    expect(onOpenTrace).toHaveBeenLastCalledWith("a2f2a89910f21f43e");
    fireEvent.click(screen.getByRole("button", { name: "Open trace for orchestrator" }));
    expect(onOpenTrace).toHaveBeenLastCalledWith("main");
    // No tab: no session section, so no "Open trace" link there; the lanes are the way in.
    expect(screen.queryByRole("button", { name: "Open trace" })).toBeNull();
  });
});
