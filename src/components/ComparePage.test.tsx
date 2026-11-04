// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { CompareOut, CompareRow } from "../flight/types";

const flightCompare = vi.fn();
const runOverview = vi.fn();
vi.mock("../flight/api", () => ({ flightCompare: (...args: unknown[]) => flightCompare(...args) }));
vi.mock("../run/api", () => ({ runOverview: () => runOverview() }));

const { ComparePage } = await import("./ComparePage");

const run = (id: string, folder = "/w/app", outcome = "done") => ({
  runId: id,
  runDir: `/d/runs/${id}`,
  folder,
  createdAt: "2026-11-05T10:00:00Z",
  stage: outcome,
  updatedAt: null,
  title: `Title ${id}`,
  record: { outcome },
});

const row = (group: string, key: string, a: CompareRow["a"], b: CompareRow["b"], extra: Partial<CompareRow> = {}): CompareRow => ({
  group,
  key,
  a,
  b,
  differs: a !== b,
  ...extra,
});

const answer = (over: Partial<CompareOut> = {}): CompareOut => ({
  a: { runId: "r-a", runDir: "/d/runs/r-a", title: "Title r-a", outcome: "done", indexed: true, off: false },
  b: { runId: "r-b", runDir: "/d/runs/r-b", title: "Title r-b", outcome: "failed", indexed: true, off: false },
  rows: [
    row("stages", "building", 10, 10),
    row("tools", "Bash", 1, 1, { aEvent: "ev_a1", bEvent: "ev_b1" }),
    row("failures", "tool.blocked:run-file-write", 0, 1, { bEvent: "ev_block" }),
    row("retries", "PR2", 0, 2, { bEvent: "ev_retry" }),
    row("files", "/w/a.rs", 1, 1),
    row("tokens", "input", 100, 100),
    row("windowShare", "fiveHour", null, null),
  ],
  divergence: 2,
  progress: { current: 4, total: 4 },
  ...over,
});

const noop = () => {};
const open = async (props: Partial<Parameters<typeof ComparePage>[0]> = {}) => {
  render(<ComparePage initialA="/d/runs/r-a" initialB="/d/runs/r-b" onOpenTrace={noop} onClose={noop} {...props} />);
  await screen.findByRole("table", { name: "Compare grid" });
};

beforeEach(() => {
  flightCompare.mockReset();
  flightCompare.mockResolvedValue(answer());
  runOverview.mockReset();
  runOverview.mockResolvedValue({
    roles: [],
    runs: [run("r-a"), run("r-b", "/w/app", "failed"), run("r-c", "/w/other"), run("r-run", "/w/app", "building")],
  });
});
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe("ComparePage", () => {
  it("shows the first difference, marks exactly one row, and opens scrolled to it", async () => {
    const scroll = vi.fn();
    Element.prototype.scrollIntoView = scroll;
    await open();
    expect(flightCompare).toHaveBeenCalledWith("/d/runs/r-a", "/d/runs/r-b");
    expect(screen.getByTestId("compare-first").textContent).toBe(
      "First difference: Failures, guard blocks (run-file-write) (0 vs 1)",
    );
    const grid = screen.getByRole("table", { name: "Compare grid" });
    const marked = grid.querySelectorAll('[aria-current="true"]');
    expect(marked).toHaveLength(1);
    expect(marked[0]!.getAttribute("aria-label")).toContain("run A 0, run B 1, differs");
    expect(marked[0]!.textContent).toContain("guard blocks");
    await waitFor(() => expect(scroll).toHaveBeenCalled());
    expect((scroll.mock.contexts as Element[]).some((el) => el === marked[0])).toBe(true);
    // The groups come in the fixed order, a null value reads "not recorded", and equal rows are muted.
    const headings = [...grid.querySelectorAll("tr.compare-group")].map((r) => r.textContent);
    expect(headings).toEqual([
      "Active minutes per stage",
      "Tool calls",
      "Failures",
      "Retries",
      "Files touched",
      "Tokens",
      "Window share",
    ]);
    expect(within(grid).getAllByText("not recorded")).toHaveLength(2);
    expect(grid.querySelectorAll("tr.same").length).toBe(5);
    expect(grid.querySelectorAll("tr.differs").length).toBe(2);
  });

  it("moves between the differing rows with n and p and the buttons, and disables them at the ends", async () => {
    Element.prototype.scrollIntoView = vi.fn();
    await open();
    const prev = screen.getByRole("button", { name: "Previous difference" }) as HTMLButtonElement;
    const next = screen.getByRole("button", { name: "Next difference" }) as HTMLButtonElement;
    const page = screen.getByRole("region", { name: "Compare runs" });
    expect(prev.disabled).toBe(true);
    expect(next.disabled).toBe(false);
    const current = () => [...page.querySelectorAll('tr[data-current="true"]')].map((r) => r.textContent);
    expect(current()[0]).toContain("guard blocks");
    fireEvent.keyDown(page, { key: "n" });
    expect(current()[0]).toContain("retries of PR2");
    expect(next.disabled).toBe(true);
    expect(prev.disabled).toBe(false);
    fireEvent.keyDown(page, { key: "n" });
    expect(current()[0]).toContain("retries of PR2");
    fireEvent.click(prev);
    expect(current()[0]).toContain("guard blocks");
    expect(prev.disabled).toBe(true);
    fireEvent.keyDown(page, { key: "p" });
    expect(current()[0]).toContain("guard blocks");
    // The marker stays on the Divergence while the keyboard moves.
    fireEvent.keyDown(page, { key: "n" });
    expect(page.querySelector('[aria-current="true"]')!.textContent).toContain("guard blocks");
  });

  it("refuses the same run twice without asking the backend", async () => {
    render(<ComparePage initialA="/d/runs/r-a" initialB="/d/runs/r-a" onOpenTrace={noop} onClose={noop} />);
    expect(await screen.findByTestId("compare-same")).toBeTruthy();
    expect(screen.getByTestId("compare-same").textContent).toBe("Pick two different runs.");
    expect(screen.queryByRole("table", { name: "Compare grid" })).toBeNull();
    expect(flightCompare).not.toHaveBeenCalled();
  });

  it("says when there is no difference, still shows the grid and disables the navigation", async () => {
    Element.prototype.scrollIntoView = vi.fn();
    flightCompare.mockResolvedValue(answer({ rows: [row("tools", "Bash", 1, 1)], divergence: null }));
    await open();
    expect(screen.getByTestId("compare-equal").textContent).toBe("No differences in these rows.");
    expect((screen.getByRole("button", { name: "Next difference" }) as HTMLButtonElement).disabled).toBe(true);
    expect((screen.getByRole("button", { name: "Previous difference" }) as HTMLButtonElement).disabled).toBe(true);
    expect(screen.queryByTestId("compare-first")).toBeNull();
  });

  it("says a run is not indexed yet, with the meter, while the other side still shows", async () => {
    flightCompare.mockResolvedValue(
      answer({
        rows: [],
        divergence: null,
        b: { runId: "r-b", runDir: "/d/runs/r-b", title: "Title r-b", outcome: "failed", indexed: false, off: false },
        progress: { current: 3, total: 9 },
      }),
    );
    render(<ComparePage initialA="/d/runs/r-a" initialB="/d/runs/r-b" onOpenTrace={noop} onClose={noop} />);
    const note = await screen.findByTestId("compare-not-indexed");
    expect(note.textContent).toContain("Run r-b is not indexed yet (Indexing 3 of 9 runs).");
    expect(within(note).getByRole("meter")).toBeTruthy();
    expect(screen.getByTestId("compare-side-r-a").textContent).toContain("Title r-a");
    expect(screen.queryByRole("table", { name: "Compare grid" })).toBeNull();
  });

  it("says AgentFlight is off for a side's account and reads no rows", async () => {
    flightCompare.mockResolvedValue(
      answer({
        rows: [],
        divergence: null,
        a: { runId: "r-a", runDir: "/d/runs/r-a", title: "Title r-a", outcome: "done", indexed: true, off: true },
      }),
    );
    render(<ComparePage initialA="/d/runs/r-a" initialB="/d/runs/r-b" onOpenTrace={noop} onClose={noop} />);
    const note = await screen.findByTestId("compare-off");
    expect(note.textContent).toContain("AgentFlight is off for this account (Settings › Activity).");
    expect(screen.queryByRole("table", { name: "Compare grid" })).toBeNull();
  });

  it("opens a run's Trace on the first Event of a clicked value, and gives record numbers no link", async () => {
    Element.prototype.scrollIntoView = vi.fn();
    const onOpenTrace = vi.fn();
    await open({ onOpenTrace });
    const grid = screen.getByRole("table", { name: "Compare grid" });
    const blocks = within(grid).getByRole("row", { name: /guard blocks/ });
    fireEvent.click(within(blocks).getByRole("button", { name: /Open the trace of r-b/ }));
    expect(onOpenTrace).toHaveBeenCalledWith("/d/runs/r-b", "ev_block");
    expect(within(blocks).queryByRole("button", { name: /Open the trace of r-a/ })).toBeNull();
    const tokens = within(grid).getByRole("row", { name: /input, run A 100/ });
    expect(within(tokens).queryAllByRole("button")).toHaveLength(0);
  });

  it("lists finished runs only, the same folder first, and compares when both are picked", async () => {
    flightCompare.mockClear();
    render(<ComparePage onOpenTrace={noop} onClose={noop} />);
    await waitFor(() => expect(runOverview).toHaveBeenCalled());
    const a = (await screen.findByLabelText("Run A")) as HTMLSelectElement;
    const b = screen.getByLabelText("Run B") as HTMLSelectElement;
    await waitFor(() => expect(a.options.length).toBeGreaterThan(1));
    const names = [...a.options].map((o) => o.value);
    expect(names).not.toContain("/d/runs/r-run");
    expect(flightCompare).not.toHaveBeenCalled();
    fireEvent.change(a, { target: { value: "/d/runs/r-a" } });
    // With A picked, B lists A's folder first.
    expect([...b.options].map((o) => o.value).slice(1, 3)).toEqual(["/d/runs/r-a", "/d/runs/r-b"]);
    expect(flightCompare).not.toHaveBeenCalled();
    fireEvent.change(b, { target: { value: "/d/runs/r-b" } });
    await waitFor(() => expect(flightCompare).toHaveBeenCalledWith("/d/runs/r-a", "/d/runs/r-b"));
  });

  it("shows the backend's refusal", async () => {
    flightCompare.mockRejectedValue("Pick two different runs.");
    render(<ComparePage initialA="/d/runs/r-a" initialB="/d/runs/r-b" onOpenTrace={noop} onClose={noop} />);
    expect((await screen.findByTestId("compare-error")).textContent).toContain("Pick two different runs.");
  });
});
