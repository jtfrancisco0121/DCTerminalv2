// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Batch } from "../run/dispatch";

const calls = vi.hoisted(() => ({ overview: vi.fn(), remove: vi.fn(async () => undefined) }));
vi.mock("../run/dispatch", () => ({ batchOverview: calls.overview, batchRemoveRun: calls.remove }));
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }));

import { BatchOverview } from "./BatchOverview";

const batch = (id: string): Batch => ({
  id,
  createdAt: "t",
  waitingForYou: 1,
  rows: [
    { entryId: "q-1", label: "one", ticketIds: ["QA-1"], removed: false, runDir: "/r/1", stage: "plan-approval", waitingFor: "plan-approval", prUrl: null, ci: null },
    { entryId: "q-2", label: "two", ticketIds: ["QA-2"], removed: false, runDir: "/r/2", stage: "ship-approval", waitingFor: null, prUrl: "https://github.com/o/r/pull/9", ci: "pass" },
    { entryId: "q-3", label: "three", ticketIds: ["QA-3"], removed: true, runDir: "/r/3", stage: "building", waitingFor: "plan-approval", prUrl: null, ci: null },
    { entryId: "q-4", label: "four", ticketIds: ["QA-4"], removed: false, runDir: null, stage: "queued", waitingFor: null, prUrl: null, ci: null },
  ],
});

beforeEach(() => {
  calls.overview.mockReset();
  calls.overview.mockResolvedValue([batch("b-1"), batch("b-2")]);
});
afterEach(cleanup);

describe("BatchOverview", () => {
  it("shows one row per run with stage, PR and CI, and the waiting-for-you count", async () => {
    render(<BatchOverview />);
    expect((await screen.findAllByTestId("batch-row-q-1"))[0]!.textContent).toContain("waiting for you (plan)");
    const withPr = screen.getAllByTestId("batch-row-q-2")[0]!;
    expect(withPr.textContent).toContain("PR");
    expect(withPr.textContent).toContain("CI passing");
    expect(screen.getAllByTestId("batch-row-q-4")[0]!.textContent).toContain("queued");
    expect(screen.getByTestId("batch-waiting").textContent).toContain("2 waiting for you");
  });

  it("strikes a removed run through and does not count it as waiting", async () => {
    render(<BatchOverview />);
    const row = (await screen.findAllByTestId("batch-row-q-3"))[0]!;
    expect(row.style.textDecoration).toBe("line-through");
    expect(row.textContent).toContain("removed from the batch");
    expect(row.textContent).not.toContain("waiting for you");
  });

  it("filters by batch and removes a run from its batch", async () => {
    render(<BatchOverview />);
    await screen.findAllByTestId("batch-row-q-1");
    fireEvent.change(screen.getByLabelText("Batch"), { target: { value: "b-2" } });
    expect(screen.queryByTestId("batch-b-1")).toBeNull();
    expect(screen.getByTestId("batch-b-2")).toBeTruthy();
    fireEvent.click(screen.getAllByText("Remove from batch")[0]!);
    await waitFor(() => expect(calls.remove).toHaveBeenCalledWith("b-2", "q-1"));
  });

  it("shows nothing when there is no batch", async () => {
    calls.overview.mockResolvedValue([]);
    const { container } = render(<BatchOverview />);
    await waitFor(() => expect(calls.overview).toHaveBeenCalled());
    expect(container.querySelector("[data-testid=batches]")).toBeNull();
  });
});
