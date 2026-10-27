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

const personal = {
  id: "main",
  accountId: "default",
  accountName: "Personal",
  title: "DCTerminalv2 — Personal",
  config: { path: "/h/.claude", display: "~/.claude", source: "default", exists: true },
  accountMissing: false,
};
let context = personal;
const setWindowAccount = vi.fn(async (accountId: string) => {
  context = { ...personal, accountId };
  return context;
});

vi.mock("./bridge", () => ({
  windowContext: vi.fn(async () => context),
  setWindowAccount: (accountId: string) => setWindowAccount(accountId),
  getSettings: vi.fn(async () => ({
    terminal: { shell: "", fontSize: 14, optionAsMeta: true, copyOnSelect: false },
    ui: { theme: "github-dark" },
    claude: { accounts: [{ id: "default", name: "Personal" }] },
    accounts: [
      {
        id: "default",
        name: "Personal",
        config: { path: "/h/.claude", display: "~/.claude", source: "default", exists: true },
      },
    ],
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
  focusTab: vi.fn(async () => null),
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}) }));
// The terminal itself is xterm in a real webview; here it is a marker.
vi.mock("./components/TerminalView", () => ({
  TerminalView: ({ tab: shown }: { tab: TabSummary }) => <div data-testid="terminal">{shown.label}</div>,
  destroyTerminal: vi.fn(),
}));
vi.mock("./terminal/park", () => ({
  focusParkedTerminal: vi.fn(),
  requestTerminalSearch: vi.fn(),
  setCopyOnSelect: vi.fn(),
  setOptionAsMeta: vi.fn(),
  setTerminalTheme: vi.fn(),
}));
vi.mock("./run/observe", async () => {
  const actual = await vi.importActual<typeof import("./run/observe")>("./run/observe");
  return {
    ...actual,
    observeTabs: vi.fn(async () => ({})),
    observeTab: vi.fn(async () => Promise.reject(new Error("no hook state"))),
    claudeVersionStatus: vi.fn(async () => ({ version: null, range: "ok", min: "", maxTested: "", selftest: null, selftestWanted: false })),
  };
});
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn(async () => null) }));
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn(async () => {}) }));

const runTab: TabSummary = { ...tab, id: "tab_run", label: "Eagle-Eye · ws", cwd: "/w/ws", runDir: "/d/runs/r-1", role: "eagle-eye" };
const startRun = vi.fn(async () => {
  state = { ...state, activeTabId: runTab.id, tabs: [runTab] };
  return { tab: runTab, runDir: runTab.runDir };
});
vi.mock("./run/api", () => ({
  runOverview: vi.fn(async () => ({
    pluginDir: "/app/plugin",
    pluginVersion: "abc123",
    roles: [
      { id: "planner", label: "Planner" },
      { id: "onboard", label: "Onboard" },
    ],
    runs: [],
  })),
  detectFolderRepos: vi.fn(async () => ({ kind: "repo", repos: [{ name: "ws", path: "/w/ws", check: "npm test" }] })),
  startRun: (...args: unknown[]) => startRun(...(args as [])),
  resumeRun: vi.fn(),
  readRun: vi.fn(async () => ({
    runId: "r-1",
    runDir: "/d/runs/r-1",
    folder: "/w/ws",
    hasStatus: true,
    statusProblem: null,
    stage: "planning",
    waitingFor: null,
    pausedReason: null,
    error: null,
    updatedAt: null,
    plan: null,
    prs: [],
    shipNote: null,
    timeline: [],
    hasReport: false,
  })),
  rerunChecks: vi.fn(async () => {}),
  prWorktreeChecks: vi.fn(async () => []),
  prWorktreeAction: vi.fn(async () => null),
  runWorktrees: vi.fn(async () => []),
  removeRunWorktrees: vi.fn(async () => []),
  readRunFile: vi.fn(async () => ""),
  memoryStatus: vi.fn(async () => ({ available: false, problem: "memory.sh is not in this plugin yet" })),
  readMemoryFile: vi.fn(async () => ""),
}));

const { default: App } = await import("./App");

describe("App", () => {
  it("an empty window starts with New run; a plain tab is one click away and opens its terminal", async () => {
    render(<App />);
    expect(await screen.findByRole("heading", { name: "New run" })).toBeTruthy();
    expect(await screen.findByTestId("status-account")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Plain terminal tab instead" }));
    expect(await screen.findByRole("heading", { name: "New tab" })).toBeTruthy();
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
    expect(await screen.findByRole("heading", { name: "New run" })).toBeTruthy();
  });

  it("Start Eagle-Eye launches the run in a tab with its progress panel beside it", async () => {
    state = { windowId: "main", activeTabId: null, tabs: [], closedTabs: [] };
    render(<App />);
    fireEvent.change(await screen.findByPlaceholderText("/path/to/repo or folder of repos"), {
      target: { value: "/w/ws" },
    });
    expect(await screen.findByText("A git repo.")).toBeTruthy();
    expect(screen.getByText("Planner")).toBeTruthy();
    fireEvent.change(screen.getByPlaceholderText(/Describe the feature/), { target: { value: "Add multiply()" } });
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Start Eagle-Eye" }));
    });
    // Review on GitHub is on by default, and so is BMAD Auto.
    expect(startRun).toHaveBeenCalledWith("main", "/w/ws", "Add multiply()", "eagle-eye", null, "github", "auto");
    expect((await screen.findByTestId("terminal")).textContent).toBe("Eagle-Eye · ws");
    expect(await screen.findByTestId("run-stage")).toBeTruthy();
    expect(screen.getByTestId("run-stage").textContent).toBe("Planning");
    state = { windowId: "main", activeTabId: null, tabs: [], closedTabs: [] };
  });

  it("the BMAD choice defaults to Auto, is remembered per folder and goes to startRun", async () => {
    state = { windowId: "main", activeTabId: null, tabs: [], closedTabs: [] };
    startRun.mockClear();
    render(<App />);
    const folder = await screen.findByPlaceholderText("/path/to/repo or folder of repos");
    fireEvent.change(folder, { target: { value: "/w/ws" } });
    const radio = (name: string) => screen.getByRole("radio", { name }) as HTMLInputElement;
    expect(radio("Auto").checked).toBe(true);
    fireEvent.click(radio("On"));
    fireEvent.change(folder, { target: { value: "/w/other" } });
    expect(radio("Auto").checked).toBe(true); // another folder: its own choice
    fireEvent.change(folder, { target: { value: "/w/ws" } });
    expect(radio("On").checked).toBe(true); // this folder remembers
    expect(await screen.findByText("A git repo.")).toBeTruthy();
    fireEvent.change(screen.getByPlaceholderText(/Describe the feature/), { target: { value: "Add multiply()" } });
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Start Eagle-Eye" }));
    });
    expect(startRun).toHaveBeenCalledWith("main", "/w/ws", "Add multiply()", "eagle-eye", null, "github", "on");
    state = { windowId: "main", activeTabId: null, tabs: [], closedTabs: [] };
    localStorage.removeItem("dcv2.bmadMode./w/ws");
  });

  // Review: a window whose account was removed ran its Claude tabs on the
  // first account instead, and saved that account's session over the tab's.
  it("a Claude tab of a removed account shows the notice, not a terminal, until an account is picked", async () => {
    context = { ...personal, accountId: "gone", accountName: "Account removed", accountMissing: true };
    state = { windowId: "main", activeTabId: tab.id, tabs: [tab], closedTabs: [] };
    render(<App />);
    expect(await screen.findByText("This window's account was removed. Pick an account.")).toBeTruthy();
    expect(screen.queryByTestId("terminal")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Pick an account" }));
    fireEvent.click(await screen.findByRole("radio"));
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Use account" }));
    });
    expect(setWindowAccount).toHaveBeenCalledWith("default");
    expect((await screen.findByTestId("terminal")).textContent).toBe("Claude · app");
    context = personal;
  });
});
