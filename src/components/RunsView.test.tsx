// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { RunListItem } from "../run/api";

const base: RunListItem = {
  runId: "r-1",
  runDir: "/d/runs/r-1",
  folder: "/w/app",
  createdAt: "2026-10-27T10:00:00Z",
  stage: "done",
  updatedAt: null,
  title: "Add multiply()",
  record: {
    outcome: "done",
    totalTokens: 1234,
    estimateUsd: 1.5,
    minutes: 35,
    prUrls: ["https://github.com/o/r/pull/1"],
    firstActivity: null,
    lastActivity: null,
  },
  prCount: 1,
  ticketIds: ["86b1aa001", "QA-129"],
  hasReport: true,
};
const runs: RunListItem[] = [
  base,
  { ...base, runId: "r-2", runDir: "/d/runs/r-2", title: "Fix login", folder: "/w/web", stage: "building", record: null, ticketIds: ["QA-7"], hasReport: false, prCount: null },
];

const resumeRun = vi.fn(async () => ({ tab: {}, runDir: "/d/runs/r-2" }));
const readRunFile = vi.fn(async () => "# Report\nall good");
const openUrl = vi.fn(async (_url: string) => {});
const revealItemInDir = vi.fn(async (_path: string) => {});
vi.mock("../run/api", () => ({
  runOverview: vi.fn(async () => ({ pluginDir: null, pluginVersion: null, roles: [], runs })),
  readRunFile: (...args: unknown[]) => readRunFile(...(args as [])),
  resumeRun: (...args: unknown[]) => resumeRun(...(args as [])),
}));
vi.mock("@tauri-apps/plugin-opener", () => ({
  openUrl: (url: string) => openUrl(url),
  revealItemInDir: (path: string) => revealItemInDir(path),
}));

const { RunsView } = await import("./RunsView");

const visible = () => screen.queryAllByTestId(/^run-r-/).map((el) => el.getAttribute("data-testid"));

describe("RunsView", () => {
  afterEach(cleanup);

  it("lists every run, labels the cost an estimate and shows — without a record", async () => {
    render(<RunsView windowId="main" onClose={() => {}} onStarted={() => {}} />);
    await screen.findByTestId("run-r-1");
    expect(visible()).toEqual(["run-r-1", "run-r-2"]);
    expect(screen.getByTestId("cost-r-1").textContent).toBe("$1.50 estimate");
    expect(screen.getByTestId("duration-r-1").textContent).toBe("35 min");
    expect(screen.getByTestId("cost-r-2").textContent).toBe("—");
    expect(screen.getByTestId("duration-r-2").textContent).toBe("—");
  });

  it("finds a run by ticket id, custom id, title or folder; the stage filter narrows", async () => {
    render(<RunsView windowId="main" onClose={() => {}} onStarted={() => {}} />);
    await screen.findByTestId("run-r-1");
    const search = screen.getByRole("searchbox", { name: "Search runs" });
    fireEvent.change(search, { target: { value: "qa-129" } });
    expect(visible()).toEqual(["run-r-1"]);
    fireEvent.change(search, { target: { value: "86b1aa001" } });
    expect(visible()).toEqual(["run-r-1"]);
    fireEvent.change(search, { target: { value: "QA-7" } });
    expect(visible()).toEqual(["run-r-2"]);
    fireEvent.change(search, { target: { value: "/w/web" } });
    expect(visible()).toEqual(["run-r-2"]);
    fireEvent.change(search, { target: { value: "nothing like it" } });
    expect(screen.getByText("No runs match.")).toBeTruthy();
    fireEvent.change(search, { target: { value: "" } });
    fireEvent.change(screen.getByRole("combobox", { name: "Stage" }), { target: { value: "building" } });
    expect(visible()).toEqual(["run-r-2"]);
  });

  it("opens the report, the PR, the folder, and resumes only unfinished runs", async () => {
    const onStarted = vi.fn();
    render(<RunsView windowId="main" onClose={() => {}} onStarted={onStarted} />);
    await screen.findByTestId("run-r-1");
    const first = within(screen.getByTestId("run-r-1"));
    const second = within(screen.getByTestId("run-r-2"));
    expect(first.queryByText("Resume")).toBeNull();
    expect(second.queryByText("Report")).toBeNull();
    fireEvent.click(first.getByText("PR 1"));
    expect(openUrl).toHaveBeenCalledWith("https://github.com/o/r/pull/1");
    fireEvent.click(first.getByText("Open folder"));
    expect(revealItemInDir).toHaveBeenCalledWith("/d/runs/r-1");
    fireEvent.click(first.getByText("Report"));
    expect(await screen.findByText(/all good/)).toBeTruthy();
    expect(readRunFile).toHaveBeenCalledWith("/d/runs/r-1", "report.md");
    fireEvent.click(second.getByText("Resume"));
    await waitFor(() => expect(onStarted).toHaveBeenCalled());
    expect(resumeRun).toHaveBeenCalledWith("main", "/d/runs/r-2");
  });
});

describe("RunsView tickets", () => {
  it("shows the tickets of each run", async () => {
    render(<RunsView windowId="main" onClose={() => {}} onStarted={() => {}} />);
    expect((await screen.findByTestId("tickets-r-1")).textContent).toContain("86b1aa001, QA-129");
    expect(screen.getByTestId("tickets-r-2").textContent).toContain("QA-7");
  });
});

describe("RunsView table and summary", () => {
  afterEach(() => {
    cleanup();
    vi.useRealTimers();
  });

  const priced = (id: string, created: string, usd: number | null, minutes: number): RunListItem => ({
    ...base,
    runId: id,
    runDir: `/d/runs/${id}`,
    createdAt: created,
    ticketIds: [],
    record: { ...base.record!, estimateUsd: usd, minutes },
  });

  async function openWith(list: RunListItem[]) {
    const api = await import("../run/api");
    vi.mocked(api.runOverview).mockResolvedValueOnce({ pluginDir: null, pluginVersion: null, roles: [], runs: list });
    render(<RunsView windowId="main" onClose={() => {}} onStarted={() => {}} />);
    await screen.findByTestId(`run-${list[0].runId}`);
  }

  it("sorts Estimate ascending, then descending, then back to newest first, unpriced last", async () => {
    await openWith([
      priced("r-a", "2026-10-31T10:00:00Z", 3, 10),
      priced("r-b", "2026-10-30T10:00:00Z", null, 20),
      priced("r-c", "2026-10-29T10:00:00Z", 1, 30),
    ]);
    const header = () => screen.getByRole("columnheader", { name: /Estimate/ });
    expect(header().getAttribute("aria-sort")).toBe("none");
    expect(visible()).toEqual(["run-r-a", "run-r-b", "run-r-c"]);
    fireEvent.click(within(header()).getByRole("button"));
    expect(header().getAttribute("aria-sort")).toBe("ascending");
    expect(visible()).toEqual(["run-r-c", "run-r-a", "run-r-b"]);
    fireEvent.click(within(header()).getByRole("button"));
    expect(header().getAttribute("aria-sort")).toBe("descending");
    expect(visible()).toEqual(["run-r-a", "run-r-c", "run-r-b"]);
    fireEvent.click(within(header()).getByRole("button"));
    expect(header().getAttribute("aria-sort")).toBe("none");
    expect(visible()).toEqual(["run-r-a", "run-r-b", "run-r-c"]);
  });

  it("sorts Duration and moves aria-sort to the clicked column", async () => {
    await openWith([priced("r-a", "2026-10-31T10:00:00Z", 3, 30), priced("r-b", "2026-10-30T10:00:00Z", 1, 10)]);
    fireEvent.click(within(screen.getByRole("columnheader", { name: /Duration/ })).getByRole("button"));
    expect(visible()).toEqual(["run-r-b", "run-r-a"]);
    fireEvent.click(within(screen.getByRole("columnheader", { name: /Estimate/ })).getByRole("button"));
    expect(screen.getByRole("columnheader", { name: /Duration/ }).getAttribute("aria-sort")).toBe("none");
    expect(screen.getByRole("columnheader", { name: /Estimate/ }).getAttribute("aria-sort")).toBe("ascending");
  });

  it("heads the table with the runs of the last 7 days, the estimate total and the median duration", async () => {
    vi.useFakeTimers({ toFake: ["Date"], now: new Date("2026-11-01T12:00:00Z") });
    await openWith([
      priced("r-a", "2026-10-31T10:00:00Z", 3, 10),
      priced("r-b", "2026-10-30T10:00:00Z", 1.25, 30),
      priced("r-c", "2026-10-30T09:00:00Z", null, 20),
      priced("r-old", "2026-10-01T10:00:00Z", 50, 500),
    ]);
    const text = screen.getByTestId("runs-summary").textContent ?? "";
    expect(text).toContain("3 runs in the last 7 days");
    expect(text).toContain("Estimate $4.25 (1 unpriced)");
    expect(text).toContain("Median duration 20 min");
    expect(within(screen.getByTestId("runs-summary")).getByText(/^Estimate/).getAttribute("title")).toBe("An estimate at API list prices, not a bill");
  });

  it("shows a dash, never $0, when no run is priced or in the window", async () => {
    vi.useFakeTimers({ toFake: ["Date"], now: new Date("2026-11-01T12:00:00Z") });
    await openWith([priced("r-old", "2026-10-01T10:00:00Z", 5, 5)]);
    const text = screen.getByTestId("runs-summary").textContent ?? "";
    expect(text).toContain("0 runs in the last 7 days");
    expect(text).toContain("Estimate —");
    expect(text).toContain("Median duration —");
    expect(text).not.toContain("$0");
  });

  it("offers Analytics only when the owner handles it", async () => {
    const onAnalytics = vi.fn();
    const { unmount } = render(<RunsView windowId="main" onClose={() => {}} onStarted={() => {}} onAnalytics={onAnalytics} />);
    await screen.findByTestId("run-r-1");
    fireEvent.click(screen.getByRole("button", { name: "Analytics" }));
    expect(onAnalytics).toHaveBeenCalled();
    unmount();
    render(<RunsView windowId="main" onClose={() => {}} onStarted={() => {}} />);
    await screen.findByTestId("run-r-1");
    expect(screen.queryByRole("button", { name: "Analytics" })).toBeNull();
  });
});
