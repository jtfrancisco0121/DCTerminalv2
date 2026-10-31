// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { RunListItem } from "../run/api";

const run = (id: string, day: number, over: Partial<RunListItem> = {}, record: Partial<NonNullable<RunListItem["record"]>> = {}): RunListItem => ({
  runId: id,
  runDir: `/d/${id}`,
  folder: "/w/app",
  createdAt: `2026-10-${day}T10:00:00Z`,
  stage: "done",
  updatedAt: null,
  title: id,
  repos: ["api"],
  record: {
    outcome: "done",
    totalTokens: 1,
    estimateUsd: 1,
    minutes: 10,
    waitMinutes: 5,
    spanMinutes: 20,
    prUrls: [],
    firstActivity: null,
    lastActivity: null,
    ...record,
  },
  ...over,
});
let runs: RunListItem[] = [];
vi.mock("../run/api", () => ({
  runOverview: vi.fn(async () => ({ pluginDir: null, pluginVersion: null, roles: [], runs })),
}));

const { RunAnalytics } = await import("./RunAnalytics");

describe("RunAnalytics", () => {
  afterEach(cleanup);

  it("draws three charts for two finished runs and captions the cost as an estimate", async () => {
    runs = [run("a", 20), run("b", 21)];
    render(<RunAnalytics onClose={() => {}} />);
    expect(await screen.findAllByRole("img")).toHaveLength(3);
    expect(screen.getByText("Estimates at API list prices")).toBeTruthy();
    expect(screen.queryByText(/not shown/)).toBeNull();
  });

  it("says how many runs a chart leaves out", async () => {
    runs = [run("a", 20), run("b", 21), run("c", 22, {}, { estimateUsd: null, waitMinutes: null, spanMinutes: null })];
    render(<RunAnalytics onClose={() => {}} />);
    await screen.findAllByRole("img");
    expect(screen.getByText(/1 not shown: no price/)).toBeTruthy();
    expect(screen.getByText(/1 not shown: no wait figure/)).toBeTruthy();
  });

  it("shows a sentence instead of a chart with fewer than two runs", async () => {
    runs = [run("a", 20)];
    render(<RunAnalytics onClose={() => {}} />);
    await screen.findByRole("region", { name: "Run analytics" });
    expect(await screen.findAllByText(/at least two finished runs/)).toHaveLength(3);
    expect(screen.queryByRole("img")).toBeNull();
  });

  it("the folder and repo filters narrow the series", async () => {
    runs = [run("a", 20), run("b", 21), run("c", 22, { folder: "/w/other", repos: ["web"] })];
    render(<RunAnalytics onClose={() => {}} />);
    const charts = () => screen.queryAllByRole("img").map((el) => el.getAttribute("aria-label") ?? "");
    await screen.findAllByRole("img");
    expect(charts()[1]).toContain("3 runs");
    fireEvent.change(screen.getByRole("combobox", { name: "Folder" }), { target: { value: "/w/app" } });
    expect(charts()[1]).toContain("2 runs");
    fireEvent.change(screen.getByRole("combobox", { name: "Repo" }), { target: { value: "web" } });
    expect(screen.queryAllByRole("img")).toHaveLength(0);
    expect(within(screen.getByRole("region", { name: "Run analytics" })).getAllByText(/at least two/)).toHaveLength(3);
  });

  it("Close calls back", async () => {
    runs = [];
    const onClose = vi.fn();
    render(<RunAnalytics onClose={onClose} />);
    fireEvent.click(await screen.findByRole("button", { name: "Close" }));
    expect(onClose).toHaveBeenCalled();
  });
});
