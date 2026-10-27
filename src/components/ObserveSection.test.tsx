// @vitest-environment jsdom
import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { ClaudeVersionStatus, TabObservation } from "../run/observe";
import { ClaudeVersionBannerView } from "./ClaudeVersionBanner";
import { ObserveView } from "./ObserveSection";

const tokens = { input: 10, output: 5, cacheWrite5m: 0, cacheWrite1h: 0, cacheRead: 1000 };
const seen: TabObservation = {
  hook: { activity: "needs-you", lastEvent: "PermissionRequest", updatedMs: 1, subagentsRunning: 2, toolRunning: false, sessionId: null },
  hooksActive: true,
  hooksInstalled: true,
  transcriptFound: true,
  context: { model: "claude-opus-5-5", used: 650_000, window: 1_000_000, percent: 65, warn: true },
  subagents: [
    {
      toolUseId: "t1",
      agentId: "a1",
      agentType: "dcv2:implementer",
      description: "Build PR1",
      status: "running",
      background: true,
      startedMs: 1,
      finishedMs: null,
    },
  ],
  usage: {
    stages: [{ name: "building", tokens, totalTokens: 1015, costUsd: 1.234, minutes: 12.5 }],
    agents: [{ name: "orchestrator", tokens, totalTokens: 1015, costUsd: 1.234, minutes: 3 }],
    totalTokens: 1015,
    totalCostUsd: 1.234,
    minutes: 12.5,
    messages: 3,
  },
  stall: { minutes: 12, threshold: 10 },
};

describe("ObserveView", () => {
  it("shows state, context warning, sub-agents and the cost estimate", () => {
    render(<ObserveView seen={seen} nudging={false} onNudge={vi.fn()} onResume={vi.fn()} />);
    expect(screen.getByTestId("tab-activity").textContent).toContain("needs you");
    expect(screen.getByTestId("tab-activity").textContent).toContain("2 sub-agents running");
    expect(screen.getByTestId("context-meter").textContent).toContain("65%");
    expect(screen.getByTestId("context-meter").textContent).toContain("over 60%");
    expect(screen.getByText(/implementer · Build PR1 — running/)).toBeTruthy();
    expect(screen.getByTestId("run-cost").textContent).toContain("$1.23 estimate");
    expect(screen.getByTestId("run-cost").querySelector("summary")?.textContent).toContain("This session:");
    expect(screen.getByTestId("run-cost").textContent).not.toMatch(/run total/i);
  });

  it("says how long the run has waited for you, only once the watchdog threshold passed", () => {
    const { rerender } = render(<ObserveView seen={seen} nudging={false} onNudge={vi.fn()} onResume={vi.fn()} />);
    expect(screen.queryByTestId("run-waiting")).toBeNull();
    rerender(
      <ObserveView
        seen={{ ...seen, waiting: { minutes: 14, threshold: 10 } }}
        nudging={false}
        onNudge={vi.fn()}
        onResume={vi.fn()}
      />,
    );
    expect(screen.getByTestId("run-waiting").textContent).toBe("Waiting for you for 14 min.");
  });

  it("offers Nudge and Resume only on a stall, and nudges only on click", () => {
    const onNudge = vi.fn();
    const onResume = vi.fn();
    const working = { ...seen, hook: { ...seen.hook!, activity: "working" as const } };
    const { rerender } = render(<ObserveView seen={working} nudging={false} onNudge={onNudge} onResume={onResume} />);
    expect(screen.getByTestId("run-stall").textContent).toContain("No progress for 12 min");
    expect(onNudge).not.toHaveBeenCalled();
    fireEvent.click(screen.getByText("Nudge"));
    fireEvent.click(screen.getByText("Resume"));
    expect(onNudge).toHaveBeenCalledTimes(1);
    expect(onResume).toHaveBeenCalledTimes(1);
    rerender(<ObserveView seen={{ ...seen, stall: null }} nudging={false} onNudge={onNudge} onResume={onResume} />);
    expect(screen.queryByTestId("run-stall")).toBeNull();
  });

  it("hides Nudge while Claude needs you and asks first when the state is unknown", () => {
    const onNudge = vi.fn();
    const { rerender } = render(<ObserveView seen={seen} nudging={false} onNudge={onNudge} onResume={vi.fn()} />);
    expect(screen.queryByText("Nudge")).toBeNull();
    const unknown = { ...seen, hook: null, hooksActive: false, hooksInstalled: false };
    rerender(<ObserveView seen={unknown} nudging={false} onNudge={onNudge} onResume={vi.fn()} />);
    expect(screen.getByTestId("hooks-missing").textContent).toContain("dcv2-hook not found");
    fireEvent.click(screen.getByText("Nudge"));
    expect(onNudge).not.toHaveBeenCalled();
    fireEvent.click(screen.getByText("Type it"));
    expect(onNudge).toHaveBeenCalledTimes(1);
  });
});

describe("ClaudeVersionBannerView", () => {
  const ok: ClaudeVersionStatus = {
    version: "2.1.287 (Claude Code)",
    range: "ok",
    min: "2.1.287",
    maxTested: "2.1.287",
    selftest: { version: "2.1.287", ok: true, pluginLoaded: true, agentSpawned: true, agentCompleted: true, finished: true, at: "x", detail: null, seconds: 7 },
    selftestWanted: false,
  };

  it("stays hidden in range with a passed self-test", () => {
    const { container } = render(<ClaudeVersionBannerView status={ok} running={false} problem={null} onSelftest={vi.fn()} />);
    expect(container.textContent).toBe("");
  });

  it("warns out of range and offers the self-test on one click", () => {
    const onSelftest = vi.fn();
    render(
      <ClaudeVersionBannerView
        status={{ ...ok, version: "2.2.0", range: "above", selftestWanted: true }}
        running={false}
        problem={null}
        onSelftest={onSelftest}
      />,
    );
    expect(screen.getByTestId("version-banner").textContent).toContain("newer than 2.1.287");
    expect(onSelftest).not.toHaveBeenCalled();
    fireEvent.click(screen.getByText(/Run the 30 s self-test/));
    expect(onSelftest).toHaveBeenCalledTimes(1);
  });
});
