// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Epic } from "../run/dispatch";

const calls = vi.hoisted(() => ({ overview: vi.fn(), start: vi.fn(async () => "q-1") }));
vi.mock("../run/dispatch", () => ({ epicsOverview: calls.overview, epicStartWave: calls.start }));

import { EpicsSection } from "./EpicsSection";

const epic: Epic = {
  runDir: "/d/runs/r-epic",
  runId: "r-epic",
  problem: null,
  cards: [
    { n: 1, title: "Foundations", ticketIds: ["a", "b", "c"], state: "done", reason: null },
    { n: 2, title: "Features", ticketIds: ["d", "e", "f"], state: "ready", reason: null },
    { n: 3, title: "Polish", ticketIds: ["g", "h", "i"], state: "waiting", reason: "wave 2 has not started" },
    { n: 4, title: "Docs", ticketIds: ["j", "k", "l"], state: "building", reason: null },
  ],
};

beforeEach(() => {
  calls.overview.mockReset();
  calls.overview.mockResolvedValue([epic]);
  calls.start.mockClear();
});
afterEach(cleanup);

describe("EpicsSection", () => {
  it("shows a card per wave with its state and the reason a wave waits", async () => {
    render(<EpicsSection />);
    expect((await screen.findByTestId("wave-r-epic-1")).textContent).toContain("done");
    expect(screen.getByTestId("wave-r-epic-3").textContent).toContain("waiting · wave 2 has not started");
    expect(screen.getByTestId("wave-r-epic-4").textContent).toContain("building");
  });

  it("offers Start wave only on a ready wave", async () => {
    render(<EpicsSection />);
    await screen.findByTestId("wave-r-epic-2");
    expect(screen.getAllByText("Start wave")).toHaveLength(1);
    fireEvent.click(screen.getByText("Start wave"));
    await waitFor(() => expect(calls.start).toHaveBeenCalledWith("/d/runs/r-epic", 2));
  });

  it("an invalid waves file shows the epic as invalid and offers nothing", async () => {
    calls.overview.mockResolvedValue([{ ...epic, problem: "waves file invalid: wave 1 has no ticket", cards: [] }]);
    render(<EpicsSection />);
    expect((await screen.findByTestId("epic-problem-r-epic")).textContent).toContain("waves file invalid");
    expect(screen.queryByText("Start wave")).toBeNull();
  });
});
