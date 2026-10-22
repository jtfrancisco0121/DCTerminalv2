// @vitest-environment jsdom
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { AppStateSnapshot, TabSummary } from "./bridge";

const tab: TabSummary = {
  id: "tab_1",
  label: "Claude · app",
  customLabel: false,
  cwd: "/w/app",
  kind: "claude",
  folderStatus: "ok",
  hasSession: false,
};
let state: AppStateSnapshot = { windowId: "main", activeTabId: null, tabs: [], closedTabs: [] };
const newTab = vi.fn(async () => {
  state = { ...state, activeTabId: tab.id, tabs: [tab] };
  return tab;
});
const closeTab = vi.fn(async () => {
  state = { ...state, activeTabId: null, tabs: [] };
  return state;
});

vi.mock("./bridge", () => ({
  windowContext: vi.fn(async () => ({
    id: "main",
    accountId: "default",
    accountName: "Personal",
    title: "DCTerminalv2 — Personal",
    config: { path: "/h/.claude", display: "~/.claude", source: "default", exists: true },
  })),
  getSettings: vi.fn(async () => ({
    terminal: { shell: "", fontSize: 14, optionAsMeta: true, copyOnSelect: false },
    ui: { theme: "github-dark" },
    claude: { accounts: [{ id: "default", name: "Personal" }] },
    accounts: [],
    configDirEnv: null,
  })),
  getAppState: vi.fn(async () => state),
  getClaudeUsage: vi.fn(async () => ({ configDir: "/h/.claude", windows: [] })),
  newTab: (...args: unknown[]) => newTab(...(args as [])),
  closeTab: (...args: unknown[]) => closeTab(...(args as [])),
  selectTab: vi.fn(async () => {}),
  renameTab: vi.fn(async () => tab),
  reopenClosedTab: vi.fn(async () => tab),
  openAccountWindow: vi.fn(async () => "win-2"),
  ptySetScheme: vi.fn(async () => {}),
}));
// The terminal itself is xterm in a real webview; here it is a marker.
vi.mock("./components/TerminalView", () => ({
  TerminalView: ({ tab: shown }: { tab: TabSummary }) => <div data-testid="terminal">{shown.label}</div>,
  destroyTerminal: vi.fn(),
}));
vi.mock("./terminal/park", () => ({
  requestTerminalSearch: vi.fn(),
  setCopyOnSelect: vi.fn(),
  setOptionAsMeta: vi.fn(),
  setTerminalTheme: vi.fn(),
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn(async () => null) }));

const { default: App } = await import("./App");

describe("App", () => {
  it("an empty window starts with the new-tab form, and a tab opens its terminal", async () => {
    render(<App />);
    expect(await screen.findByRole("heading", { name: "New tab" })).toBeTruthy();
    expect(await screen.findByTestId("status-account")).toBeTruthy();
    fireEvent.change(screen.getByPlaceholderText("/path/to/project"), { target: { value: "/w/app" } });
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Open tab" }));
    });
    expect(newTab).toHaveBeenCalledWith("main", "/w/app", "claude");
    expect((await screen.findByTestId("terminal")).textContent).toBe("Claude · app");
    expect(screen.getByRole("tab", { name: /Claude · app/ })).toBeTruthy();
  });

  it("closes a tab whose terminal is not running without asking", async () => {
    state = { windowId: "main", activeTabId: tab.id, tabs: [tab], closedTabs: [] };
    render(<App />);
    fireEvent.click(await screen.findByRole("button", { name: "Close Claude · app" }));
    await waitFor(() => expect(closeTab).toHaveBeenCalledWith("main", "tab_1"));
    expect(await screen.findByRole("heading", { name: "New tab" })).toBeTruthy();
  });
});
