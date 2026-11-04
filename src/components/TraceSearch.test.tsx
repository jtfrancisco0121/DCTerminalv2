// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { RunHit, SearchOut } from "../flight/types";

const flightSearch = vi.fn();
vi.mock("../flight/api", () => ({ flightSearch: (...args: unknown[]) => flightSearch(...args) }));

const { TraceSearch, filterOf, pillsOf, EMPTY_FORM } = await import("./TraceSearch");

const hit = (runId: string, matches: number, over: Partial<RunHit> = {}): RunHit => ({
  runId,
  runDir: `/data/runs/${runId}`,
  title: `Run ${runId}`,
  outcome: "done",
  matches,
  firstEventId: `ev_${runId}`,
  ...over,
});
const out = (over: Partial<SearchOut> = {}): SearchOut => ({
  runs: [hit("r-2", 3), hit("r-1", 1)],
  excludedOff: 0,
  progress: { current: 5, total: 5 },
  ...over,
});

const noop = () => {};
const view = (props: Partial<Parameters<typeof TraceSearch>[0]> = {}) =>
  render(<TraceSearch initial={null} onChange={noop} onOpen={noop} onClose={noop} {...props} />);
const form = () => screen.getByRole("form", { name: "Trace filters" });
const results = () => within(screen.getByRole("listbox", { name: "Matching runs" })).getAllByRole("option");

beforeEach(() => {
  flightSearch.mockReset();
  flightSearch.mockResolvedValue(out());
});
afterEach(cleanup);

describe("TraceSearch", () => {
  it("sends the structured filter on submit, never while typing, and the 'During review' shortcut sets inReview", async () => {
    view();
    fireEvent.change(screen.getByLabelText("Kind"), { target: { value: "check.end" } });
    fireEvent.change(screen.getByLabelText("Name contains"), { target: { value: "cargo test" } });
    fireEvent.change(screen.getByLabelText("Status"), { target: { value: "failed" } });
    expect(flightSearch).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "During review" }));
    expect(screen.getByRole("button", { name: "During review" }).getAttribute("aria-pressed")).toBe("true");
    fireEvent.submit(form());
    await screen.findByRole("listbox", { name: "Matching runs" });
    expect(flightSearch).toHaveBeenCalledTimes(1);
    expect(flightSearch).toHaveBeenCalledWith({
      kind: "check.end",
      nameContains: "cargo test",
      status: ["failed"],
      inReview: true,
    });
  });

  it("filterOf leaves empty fields out and sends only values", () => {
    expect(filterOf(EMPTY_FORM)).toEqual({});
    expect(filterOf({ ...EMPTY_FORM, nameContains: "  x  ", prState: "fixing", from: "2026-11-01", text: "a'; DROP" })).toEqual({
      nameContains: "x",
      prState: "fixing",
      from: "2026-11-01",
      text: "a'; DROP",
    });
  });

  it("shows the active filters as pills", async () => {
    view();
    fireEvent.change(screen.getByLabelText("PR state"), { target: { value: "fixing" } });
    fireEvent.change(screen.getByLabelText("PR role"), { target: { value: "pr-reviewer" } });
    fireEvent.submit(form());
    const pills = await screen.findByRole("list", { name: "Active filters" });
    expect(pills.textContent).toContain("PR state: fixing");
    expect(pills.textContent).toContain("PR role: pr-reviewer");
    expect(pillsOf({ status: ["fail"], inReview: true }).map((p) => p.text)).toEqual(["Status: fail", "During review"]);
  });

  it("lists runs with their match counts, and Enter on a result opens it with the first Event id", async () => {
    const onOpen = vi.fn();
    view({ onOpen });
    fireEvent.submit(form());
    const list = await screen.findByRole("listbox", { name: "Matching runs" });
    const options = within(list).getAllByRole("option");
    expect(options.map((o) => o.textContent)).toEqual([
      expect.stringContaining("3 matches"),
      expect.stringContaining("1 match"),
    ]);
    expect(options[0]!.textContent).toContain("Run r-2");
    fireEvent.keyDown(list, { key: "ArrowDown" });
    expect(list.getAttribute("aria-activedescendant")).toBe("search-run-r-1");
    fireEvent.keyDown(list, { key: "ArrowUp" });
    fireEvent.keyDown(list, { key: "Enter" });
    expect(onOpen).toHaveBeenCalledTimes(1);
    expect(onOpen.mock.calls[0]![0]).toMatchObject({ runDir: "/data/runs/r-2", firstEventId: "ev_r-2" });
    fireEvent.click(options[1]!);
    expect(onOpen.mock.calls[1]![0].firstEventId).toBe("ev_r-1");
  });

  it("says when nothing is indexed yet, and when nothing matches", async () => {
    flightSearch.mockResolvedValue(out({ runs: [], progress: { current: 0, total: 3 } }));
    view();
    fireEvent.submit(form());
    expect((await screen.findByTestId("search-empty")).textContent).toBe(
      "No runs indexed yet. Indexing runs in the background.",
    );
    expect(screen.getByTestId("search-progress").textContent).toContain("Indexing 0 of 3 runs");
    flightSearch.mockResolvedValue(out({ runs: [] }));
    fireEvent.submit(form());
    await waitFor(() => expect(screen.getByTestId("search-empty").textContent).toBe("No runs match."));
    expect(screen.queryByTestId("search-progress")).toBeNull();
  });

  it("shows index progress with a meter while the index is behind", async () => {
    flightSearch.mockResolvedValue(out({ progress: { current: 2, total: 8 } }));
    view();
    fireEvent.submit(form());
    const note = await screen.findByTestId("search-progress");
    expect(note.textContent).toContain("Indexing 2 of 8 runs");
    expect(within(note).getByRole("meter").getAttribute("aria-valuenow")).toBe("25");
  });

  it("counts the runs excluded because AgentFlight is off for their account", async () => {
    flightSearch.mockResolvedValue(out({ excludedOff: 2 }));
    view();
    fireEvent.submit(form());
    expect((await screen.findByTestId("search-excluded")).textContent).toBe(
      "2 runs excluded: AgentFlight is off for their account",
    );
  });

  it("shows the error and keeps the previous results", async () => {
    view();
    fireEvent.submit(form());
    await screen.findByRole("listbox", { name: "Matching runs" });
    flightSearch.mockRejectedValue("database is locked");
    fireEvent.submit(form());
    expect((await screen.findByTestId("search-error")).textContent).toContain("database is locked");
    expect(results()).toHaveLength(2);
  });

  it("runs one search at a time: a second submit waits for the first", async () => {
    let finish: (v: SearchOut) => void = () => {};
    flightSearch.mockReturnValue(new Promise<SearchOut>((r) => (finish = r)));
    view();
    fireEvent.submit(form());
    fireEvent.submit(form());
    expect(flightSearch).toHaveBeenCalledTimes(1);
    expect((screen.getByRole("button", { name: "Searching…" }) as HTMLButtonElement).disabled).toBe(true);
    finish(out());
    await screen.findByRole("listbox", { name: "Matching runs" });
    expect(flightSearch).toHaveBeenCalledTimes(1);
  });

  it("keeps its form and results through onChange, restores them from initial, and Clear empties them", async () => {
    const onChange = vi.fn();
    const first = view({ onChange });
    fireEvent.change(screen.getByLabelText("Repo"), { target: { value: "toy" } });
    fireEvent.submit(form());
    await screen.findByRole("listbox", { name: "Matching runs" });
    const saved = onChange.mock.calls[onChange.mock.calls.length - 1]![0];
    expect(saved.filter).toEqual({ repo: "toy" });
    expect(saved.out.runs).toHaveLength(2);
    first.unmount();
    view({ initial: saved, onChange });
    expect((screen.getByLabelText("Repo") as HTMLInputElement).value).toBe("toy");
    expect(results()).toHaveLength(2);
    expect(flightSearch).toHaveBeenCalledTimes(1);
    fireEvent.click(screen.getByRole("button", { name: "Clear" }));
    expect(screen.queryByRole("listbox")).toBeNull();
    expect((screen.getByLabelText("Repo") as HTMLInputElement).value).toBe("");
    expect(onChange).toHaveBeenLastCalledWith({ form: EMPTY_FORM, filter: null, out: null });
  });

  it("starts with a hint and searches nothing until asked", () => {
    view();
    expect(screen.getByText(/Nothing searches while you type/)).toBeTruthy();
    expect(flightSearch).not.toHaveBeenCalled();
  });
});
