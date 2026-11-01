// @vitest-environment jsdom
import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { ConfigDirInfo } from "../bridge";
import { clearUiErrors, reportError } from "../uiErrors";
import { formatReset, type RateWindow, type WindowHistory } from "../usage/limits";
import { accountFolderWarning, attentionText, StatusBar } from "./StatusBar";

const config = (over: Partial<ConfigDirInfo> = {}): ConfigDirInfo => ({
  path: "/Users/jt/.claude-account2",
  display: "~/.claude-account2",
  source: "setting",
  exists: true,
  ...over,
});

describe("StatusBar", () => {
  it("names the window's account, the tab's folder and the usage", () => {
    const onNewWindow = vi.fn();
    const onSettings = vi.fn();
    render(
      <StatusBar
        account={{ name: "Work", config: config() }}
        folder="/Users/jt/Koneksi"
        usage={{ text: "Claude 5h 82% · resets 3:10 PM", tone: "warn", title: "limits" }}
        onNewWindow={onNewWindow}
        onSettings={onSettings}
      />,
    );
    expect(screen.getByTestId("status-account").textContent).toBe("Work · ~/.claude-account2");
    expect(screen.getByText("Koneksi").getAttribute("title")).toBe("/Users/jt/Koneksi");
    expect(screen.getByTestId("status-usage").className).toContain("status-usage-warn");
    fireEvent.click(screen.getByRole("button", { name: "New window…" }));
    fireEvent.click(screen.getByRole("button", { name: "Settings" }));
    expect(onNewWindow).toHaveBeenCalledOnce();
    expect(onSettings).toHaveBeenCalledOnce();
  });

  it("words the tooltips for the platform: glyphs on macOS, Ctrl+Shift elsewhere", () => {
    const base = { account: { name: "Work", config: config() }, onNewWindow: () => {}, onSettings: () => {} };
    const { unmount } = render(<StatusBar {...base} platform="mac" />);
    expect(screen.getByRole("button", { name: "New window…" }).getAttribute("title")).toBe("New window (⌘⇧N)");
    expect(screen.getByRole("button", { name: "Settings" }).getAttribute("title")).toBe("Settings (⌘,)");
    unmount();
    render(<StatusBar {...base} platform="windows" />);
    expect(screen.getByRole("button", { name: "New window…" }).getAttribute("title")).toBe("New window (Ctrl+Shift+N)");
    expect(screen.getByRole("button", { name: "Settings" }).getAttribute("title")).toBe("Settings (Ctrl+Shift+,)");
  });

  describe("usage meters", () => {
    const future = Math.floor(Date.now() / 1000) + 3600;
    const win = (over: Partial<RateWindow> = {}): RateWindow => ({
      rateLimitType: "five_hour",
      label: "5h",
      utilization: 42,
      resetsAt: future,
      status: "allowed",
      seenAtMs: Date.now(),
      ...over,
    });
    const hist = (n: number): WindowHistory[] => [
      {
        rateLimitType: "five_hour",
        readings: Array.from({ length: n }, (_, i) => ({ atMs: i * 600_000, utilization: 30 + i * 6, resetsAt: future })),
      },
    ];
    const bar = (windows: RateWindow[], history: WindowHistory[] = []) =>
      render(<StatusBar account={null} windows={windows} history={history} onNewWindow={() => {}} onSettings={() => {}} />);

    it("renders one toned meter per window", () => {
      bar([win(), win({ rateLimitType: "seven_day", label: "7d", utilization: 90, status: "allowed_warning" })]);
      const ok = screen.getByRole("meter", { name: "Claude 5h 42%" });
      const warn = screen.getByRole("meter", { name: "Claude 7d 90%" });
      expect(ok.getAttribute("aria-valuenow")).toBe("42");
      expect(ok.className).toContain("tone-success");
      expect(warn.className).toContain("tone-warn");
      expect(screen.getAllByRole("button", { name: /^Claude (5h|7d)/ })).toHaveLength(2);
    });

    it("draws a stale window as an unknown hatched meter, never 0", () => {
      bar([win({ resetsAt: 1 })]);
      const meter = screen.getByRole("meter", { name: /unknown/ });
      expect(meter.className).toContain("meter-unknown");
      expect(meter.hasAttribute("aria-valuenow")).toBe(false);
    });

    it("opens the sparkline on focus with the reset time, and Escape closes it", () => {
      bar([win()], hist(3));
      const button = screen.getByRole("button", { name: "Claude 5h 42%" });
      expect(screen.queryByRole("tooltip")).toBeNull();
      fireEvent.focus(button);
      const tip = screen.getByRole("tooltip");
      expect(tip.querySelector("svg polyline")).not.toBeNull();
      expect(tip.textContent).toContain(`resets ${formatReset(future)}`);
      expect(tip.textContent).toContain("rising");
      expect(button.getAttribute("aria-describedby")).toBe(tip.id);
      fireEvent.keyDown(button, { key: "Escape" });
      expect(screen.queryByRole("tooltip")).toBeNull();
      fireEvent.mouseEnter(button.parentElement!);
      expect(screen.getByRole("tooltip")).toBeTruthy();
      fireEvent.mouseLeave(button.parentElement!);
      expect(screen.queryByRole("tooltip")).toBeNull();
    });

    it("says there are not enough readings with one", () => {
      bar([win()], hist(1));
      fireEvent.focus(screen.getByRole("button", { name: "Claude 5h 42%" }));
      expect(screen.getByRole("tooltip").textContent).toContain("Not enough readings yet.");
      expect(screen.getByRole("tooltip").querySelector("svg")).toBeNull();
    });
  });

  it("words the attention control and hides it at 0", () => {
    expect(attentionText({ needsYou: 0, activeRuns: 0 })).toBeNull();
    expect(attentionText({ needsYou: 1, activeRuns: 0 })).toBe("1 needs you");
    expect(attentionText({ needsYou: 2, activeRuns: 3 })).toBe("2 need you · 3 runs");
    expect(attentionText({ needsYou: 0, activeRuns: 1 })).toBe("1 run");
    const { rerender } = render(
      <StatusBar account={null} attention={{ needsYou: 0, activeRuns: 0 }} onNewWindow={() => {}} onSettings={() => {}} />,
    );
    expect(screen.queryByTestId("status-attention")).toBeNull();
    const onAttention = vi.fn();
    rerender(
      <StatusBar
        account={null}
        attention={{ needsYou: 2, activeRuns: 3 }}
        onAttention={onAttention}
        onNewWindow={() => {}}
        onSettings={() => {}}
      />,
    );
    expect(screen.getByTestId("status-attention").textContent).toBe("2 need you · 3 runs");
    fireEvent.click(screen.getByTestId("status-attention"));
    expect(onAttention).toHaveBeenCalledOnce();
  });

  it("warns when an account's folder you set does not exist", () => {
    render(
      <StatusBar
        account={{ name: "Work", config: config({ exists: false }) }}
        onNewWindow={() => {}}
        onSettings={() => {}}
      />,
    );
    const account = screen.getByTestId("status-account");
    expect(account.textContent).toContain("folder not found");
    expect(account.getAttribute("title")).toBe("Claude config folder not found: /Users/jt/.claude-account2");
  });

  it("does not warn about a default ~/.claude that Claude Code will make", () => {
    expect(accountFolderWarning(config({ exists: false, source: "default" }))).toBeNull();
    expect(accountFolderWarning(config())).toBeNull();
  });

  it("shows a notice with its dismiss", () => {
    const onDismiss = vi.fn();
    render(
      <StatusBar
        account={null}
        message={{ text: "Close tab failed: boom", onDismiss }}
        onNewWindow={() => {}}
        onSettings={() => {}}
      />,
    );
    expect(screen.queryByTestId("status-account")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Dismiss: Close tab failed: boom" }));
    expect(onDismiss).toHaveBeenCalledOnce();
  });

  describe("recent errors", () => {
    afterEach(() => {
      clearUiErrors();
      vi.unstubAllEnvs();
    });
    const bar = () => <StatusBar account={null} onNewWindow={() => {}} onSettings={() => {}} />;

    it("hides the count at 0 and lists errors newest first with local times", () => {
      vi.stubEnv("TZ", "Asia/Manila");
      vi.useFakeTimers({ toFake: ["Date"] });
      vi.setSystemTime(new Date("2026-10-27T14:04:27Z"));
      render(bar());
      expect(screen.queryByTestId("error-count")).toBeNull();
      act(() => reportError("First source", "first failed"));
      act(() => reportError("Second source", "second failed"));
      vi.useRealTimers();
      expect(screen.getByTestId("error-count").textContent).toBe("2 errors");
      fireEvent.click(screen.getByTestId("error-count"));
      const rows = screen.getByRole("dialog", { name: "Recent errors" }).querySelectorAll("li");
      expect(rows.length).toBe(2);
      expect(rows[0].textContent).toContain("Second source");
      expect(rows[1].textContent).toContain("first failed");
      expect(rows[0].textContent).toContain("22:04 · Second source");
      expect(rows[0].querySelector(".errors-source")?.getAttribute("title")).toContain("(UTC 2026-10-27T14:04:27Z)");
    });

    it("shows at most 50 entries", () => {
      render(bar());
      act(() => {
        for (let i = 0; i < 60; i++) reportError(`src ${i}`, "boom");
      });
      fireEvent.click(screen.getByTestId("error-count"));
      expect(screen.getByRole("dialog", { name: "Recent errors" }).querySelectorAll("li").length).toBe(50);
    });

    it("Clear empties the list and closes; Escape returns focus to the count", () => {
      render(bar());
      act(() => reportError("Source", "boom"));
      const count = screen.getByTestId("error-count");
      fireEvent.click(count);
      fireEvent.keyDown(window, { key: "Escape" });
      expect(screen.queryByRole("dialog", { name: "Recent errors" })).toBeNull();
      expect(document.activeElement).toBe(count);
      fireEvent.click(count);
      fireEvent.click(screen.getByRole("button", { name: "Clear" }));
      expect(screen.queryByRole("dialog", { name: "Recent errors" })).toBeNull();
      expect(screen.queryByTestId("error-count")).toBeNull();
    });
  });
});
