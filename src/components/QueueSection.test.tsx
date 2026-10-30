// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { DispatchOverview } from "../run/dispatch";

const calls = vi.hoisted(() => ({
  overview: vi.fn(),
  move: vi.fn(async () => true),
  cancel: vi.fn(async () => undefined),
  startNow: vi.fn(async () => undefined),
  release: vi.fn(async () => undefined),
}));
vi.mock("../run/dispatch", async () => ({
  ...(await vi.importActual<typeof import("../run/dispatch")>("../run/dispatch")),
  dispatchOverview: calls.overview,
  dispatchMove: calls.move,
  dispatchCancel: calls.cancel,
  dispatchStartNow: calls.startNow,
  dispatchRelease: calls.release,
}));

import { QueueSection } from "./QueueSection";

const overview: DispatchOverview = {
  cap: 2,
  usageHoldPercent: null,
  atCap: true,
  holders: [
    { runDir: "/d/runs/r-a", runId: "r-a", stage: "building", why: "tab open" },
    { runDir: "/d/runs/r-b", runId: "r-b", stage: "plan-approval", why: "recent change" },
  ],
  queue: [
    { id: "q-1", label: "First ticket", state: "queued", reason: "2 of 2 slots are in use", accountId: "a", queuedAt: "t", batchId: null, params: { folder: "/w", request: "x" } },
    { id: "q-2", label: "Second ticket", state: "queued", reason: null, accountId: "a", queuedAt: "t", batchId: null, params: { folder: "/w", request: "y" } },
  ],
};

beforeEach(() => {
  Object.values(calls).forEach((f) => f.mockClear());
  calls.overview.mockResolvedValue(overview);
});
afterEach(cleanup);

describe("QueueSection", () => {
  it("lists the entries with their reasons and every slot holder with why it counts", async () => {
    render(<QueueSection />);
    expect((await screen.findByTestId("queue-q-1")).textContent).toContain("2 of 2 slots are in use");
    expect(screen.getByTestId("queue-q-2").textContent).toContain("starts when a slot is free");
    expect(screen.getByTestId("holder-r-a").textContent).toContain("tab open");
    expect(screen.getByTestId("holder-r-b").textContent).toContain("recent change");
    expect(screen.getByText(/2 of 2 slots in use/)).toBeTruthy();
  });

  it("Release slot, Up, Down and Cancel call the dispatcher", async () => {
    render(<QueueSection />);
    await screen.findByTestId("holder-r-a");
    fireEvent.click(screen.getAllByText("Release slot")[0]!);
    await waitFor(() => expect(calls.release).toHaveBeenCalledWith("/d/runs/r-a"));
    fireEvent.click(screen.getAllByText("Up")[1]!);
    await waitFor(() => expect(calls.move).toHaveBeenCalledWith("q-2", true));
    fireEvent.click(screen.getAllByText("Down")[0]!);
    await waitFor(() => expect(calls.move).toHaveBeenCalledWith("q-1", false));
    fireEvent.click(screen.getAllByText("Cancel")[0]!);
    await waitFor(() => expect(calls.cancel).toHaveBeenCalledWith("q-1"));
  });

  it("Start now asks first and then bypasses the cap", async () => {
    render(<QueueSection />);
    await screen.findByTestId("queue-q-1");
    fireEvent.click(screen.getAllByText("Start now")[0]!);
    expect(calls.startNow).not.toHaveBeenCalled();
    fireEvent.click(screen.getByText("Yes, start now"));
    await waitFor(() => expect(calls.startNow).toHaveBeenCalledWith("q-1"));
  });

  it("shows nothing when there is no queue and no holder", async () => {
    calls.overview.mockResolvedValue({ ...overview, queue: [], holders: [] });
    const { container } = render(<QueueSection />);
    await waitFor(() => expect(calls.overview).toHaveBeenCalled());
    expect(container.querySelector("[data-testid=queue]")).toBeNull();
  });
});
