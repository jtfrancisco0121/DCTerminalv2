// @vitest-environment jsdom
import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { TabObservation } from "../run/observe";

const observeTab = vi.fn();
vi.mock("../run/observe", async () => {
  const actual = await vi.importActual<typeof import("../run/observe")>("../run/observe");
  return { ...actual, observeTab: (id: string) => observeTab(id) };
});

const { UsagePopover } = await import("./UsagePopover");

const seen = (messages: number): TabObservation => ({
  hook: null,
  hooksActive: true,
  hooksInstalled: true,
  transcriptFound: true,
  context: null,
  subagents: [],
  usage: {
    stages: [],
    agents: [],
    totalTokens: 12_345,
    totalCostUsd: 0.5,
    minutes: 7.2,
    messages,
  },
  stall: null,
});

describe("UsagePopover", () => {
  it("shows tokens, the estimated cost (labelled) and minutes", async () => {
    observeTab.mockResolvedValue(seen(3));
    render(<UsagePopover tabId="t1" onClose={() => {}} />);
    expect((await screen.findByTestId("usage-tokens")).textContent).toBe("12k tokens");
    expect(screen.getByTestId("usage-cost").textContent).toBe("$0.50 estimate");
    expect(screen.getByTestId("usage-minutes").textContent).toBe("7 min");
    expect(observeTab).toHaveBeenCalledWith("t1");
  });

  it("says so when nothing is recorded, and closes on Escape", async () => {
    observeTab.mockResolvedValue(seen(0));
    const onClose = vi.fn();
    render(<UsagePopover tabId="t1" onClose={onClose} />);
    expect(await screen.findByText("No usage recorded for this session yet.")).toBeTruthy();
    fireEvent.keyDown(window, { key: "Escape" });
    expect(onClose).toHaveBeenCalled();
  });
});
