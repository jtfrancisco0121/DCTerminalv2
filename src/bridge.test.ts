import { beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn(async (..._args: unknown[]) => null as unknown);
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...args),
  Channel: class {
    onmessage: ((packet: unknown) => void) | null = null;
  },
}));

const bridge = await import("./bridge");

beforeEach(() => invoke.mockClear());

describe("bridge argument shapes", () => {
  it("names the window on every tab command", async () => {
    await bridge.getAppState("win-2");
    await bridge.newTab("win-2", "/w/app", "shell");
    await bridge.selectTab("win-2", "tab_1");
    await bridge.closeTab("win-2", "tab_1");
    await bridge.reopenClosedTab("win-2");
    await bridge.reopenClosedTab("win-2", "tab_9");
    await bridge.renameTab("tab_1", "Auth bug");
    expect(invoke.mock.calls).toEqual([
      ["get_app_state", { windowId: "win-2" }],
      ["new_tab", { windowId: "win-2", cwd: "/w/app", kind: "shell" }],
      ["select_tab", { windowId: "win-2", tabId: "tab_1" }],
      ["close_tab", { windowId: "win-2", tabId: "tab_1" }],
      ["reopen_closed_tab", { windowId: "win-2", tabId: null }],
      ["reopen_closed_tab", { windowId: "win-2", tabId: "tab_9" }],
      ["rename_tab", { tabId: "tab_1", label: "Auth bug" }],
    ]);
  });

  it("opens a PTY with its input apart from the output channel", async () => {
    const onOutput = bridge.createPtyChannel(() => {});
    await bridge.ptyOpen({ id: "tab_1", cols: 100, rows: 30, resume: true, onOutput });
    expect(invoke).toHaveBeenCalledWith("pty_open", {
      input: { id: "tab_1", cols: 100, rows: 30, resume: true },
      onOutput,
    });
  });

  it("passes the terminal, account and settings commands their arguments", async () => {
    await bridge.ptyWrite("tab_1", "ls\r");
    await bridge.ptyResize("tab_1", 80, 24);
    await bridge.ptyKill("tab_1");
    await bridge.ptySetScheme(true);
    await bridge.terminalResumeId("tab_1");
    await bridge.openAccountWindow("work");
    await bridge.getClaudeUsage("main");
    const claude = { claudePath: null, accounts: [{ id: "default", name: "Claude" }] };
    await bridge.setClaudeSettings(claude);
    await bridge.setUiSettings({ theme: "nord" });
    const terminal = { shell: "", fontSize: 14, optionAsMeta: true, copyOnSelect: false };
    await bridge.setTerminalSettings(terminal);
    expect(invoke.mock.calls).toEqual([
      ["pty_write", { id: "tab_1", data: "ls\r" }],
      ["pty_resize", { id: "tab_1", cols: 80, rows: 24 }],
      ["pty_kill", { id: "tab_1" }],
      ["pty_set_scheme", { light: true }],
      ["terminal_resume_id", { tabId: "tab_1" }],
      ["open_account_window", { accountId: "work" }],
      ["get_claude_usage", { windowId: "main" }],
      ["set_claude_settings", { claude }],
      ["set_ui_settings", { ui: { theme: "nord" } }],
      ["set_terminal_settings", { terminal }],
    ]);
  });

  it("calls the argument-free commands by name", async () => {
    await bridge.windowContext();
    await bridge.reopenClosedWindow();
    await bridge.getSettings();
    await bridge.claudeAccountLogins();
    await bridge.claudeStatus();
    expect(invoke.mock.calls.map((call) => call[0])).toEqual([
      "window_context",
      "reopen_closed_window",
      "get_settings",
      "claude_account_logins",
      "claude_status",
    ]);
  });
});
