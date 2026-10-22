// @vitest-environment jsdom
import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { ConfigDirInfo } from "../bridge";
import { accountFolderWarning, StatusBar } from "./StatusBar";

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
});
