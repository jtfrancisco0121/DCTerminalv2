// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { useEffect } from "react";
import { describe, expect, it, vi } from "vitest";
import { attentionSummary, type AppStateSnapshot, type TabSummary } from "./bridge";
import { clearUiErrors, uiErrors } from "./uiErrors";

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
  moveTab: vi.fn(async () => state),
  splitTab: vi.fn(async () => tab),
  closePane: vi.fn(async () => tab),
  setSplitRatio: vi.fn(async () => tab),
  recentFolders: vi.fn(async () => []),
  setTerminalSettings: vi.fn(async (terminal: unknown) => terminal),
  reopenClosedTab: vi.fn(async () => tab),
  openAccountWindow: vi.fn(async () => "win-2"),
  ptySetScheme: vi.fn(async () => {}),
  focusTab: vi.fn(async () => null),
  attentionSummary: vi.fn(async () => ({ needsYou: 0, activeRuns: 0 })),
  activeRuns: vi.fn(async () => []),
  runTabs: vi.fn(async () => []),
  terminalResumeId: vi.fn(async () => null),
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}) }));
// The window's own event handlers, by name, so a test can fire `tabs-changed`.
const windowEvents = vi.hoisted(() => new Map<string, (event: { payload: unknown }) => void>());
vi.mock("@tauri-apps/api/webviewWindow", () => ({
  getCurrentWebviewWindow: () => ({
    label: "main",
    listen: async (name: string, handler: (event: { payload: unknown }) => void) => {
      windowEvents.set(name, handler);
      return () => windowEvents.delete(name);
    },
  }),
}));
// The terminal itself is xterm in a real webview; here it is a marker.
const terminalLife = vi.hoisted(() => ({ mounts: 0, unmounts: 0 }));
vi.mock("./components/TerminalView", () => ({
  TerminalView: ({ tab: shown }: { tab: TabSummary }) => {
    // Counts mounts and unmounts: opening a page must not unmount a terminal.
    useEffect(() => {
      terminalLife.mounts += 1;
      return () => {
        terminalLife.unmounts += 1;
      };
    }, []);
    return <div data-testid="terminal">{shown.label}</div>;
  },
  destroyTerminal: vi.fn(),
}));
vi.mock("./terminal/park", () => ({
  focusParkedTerminal: vi.fn(),
  refitTerminal: vi.fn(),
  requestTerminalSearch: vi.fn(),
  setCopyOnSelect: vi.fn(),
  setCompactPaste: vi.fn(),
  setFileLinkEditor: vi.fn(),
  setOptionAsMeta: vi.fn(),
  setTerminalTheme: vi.fn(),
  ensureParkedTerminal: vi.fn(),
  measuredTerminalSize: () => ({ cols: 80, rows: 24 }),
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
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn(async () => {}), revealItemInDir: vi.fn(async () => {}) }));

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
  takeRunLink: vi.fn(async () => null),
  resolveRunLink: vi.fn(),
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

vi.mock("./flight/api", () => ({
  flightSession: vi.fn(async () => ({
    status: "ok",
    trace: "r-1",
    policy: "metadata",
    agents: [],
    failures: [],
    page: { events: [], nextCursor: null },
    notices: [],
  })),
  flightContent: vi.fn(),
  flightSearch: vi.fn(),
  flightCompare: vi.fn(),
}));

const running = new Set<string>();
const openLive = vi.hoisted(() => vi.fn(async () => "opened"));
vi.mock("./terminal/live", async (original) => ({
  ...(await original<typeof import("./terminal/live")>()),
  ptyRunning: (id: string) => running.has(id),
  openLivePty: (...args: unknown[]) => (openLive as (...a: unknown[]) => unknown)(...args),
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
    expect(startRun).toHaveBeenCalledWith("main", "/w/ws", "Add multiply()", "eagle-eye", null, "github", "auto", false, null, null, null);
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
    expect(startRun).toHaveBeenCalledWith("main", "/w/ws", "Add multiply()", "eagle-eye", null, "github", "on", false, null, null, null);
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

describe("App attention control", () => {
  it("reads the attention summary again when the detector announces something", async () => {
    const bridge = await import("./bridge");
    const events = await import("@tauri-apps/api/event");
    const summary = bridge.attentionSummary as unknown as ReturnType<typeof vi.fn>;
    const listen = events.listen as unknown as ReturnType<typeof vi.fn>;
    summary.mockResolvedValue({ needsYou: 0, activeRuns: 0 });
    render(<App />);
    expect(await screen.findByTestId("status-account")).toBeTruthy();
    expect(screen.queryByTestId("status-attention")).toBeNull();
    const calls = summary.mock.calls.length;
    summary.mockResolvedValue({ needsYou: 2, activeRuns: 1 });
    const payload = { kind: "needs-you", tabId: null, windowLabel: null, runId: null, text: "dcv2: a tab needs you" };
    const handlers = listen.mock.calls.filter(([name]) => name === "attention").map(([, fn]) => fn as (e: unknown) => void);
    await act(async () => handlers.forEach((fn) => fn({ payload })));
    expect(summary.mock.calls.length).toBeGreaterThan(calls);
    expect((await screen.findByTestId("status-attention")).textContent).toBe("2 need you · 1 run");
    fireEvent.click(screen.getByTestId("status-attention"));
    expect(await screen.findByRole("dialog", { name: "Active runs" })).toBeTruthy();
  });
});

describe("App tab cycling and font zoom", () => {
  it("Ctrl+Tab selects the next tab and wraps; Ctrl+Shift+plus raises the font size by one", async () => {
    const bridge = await import("./bridge");
    const selectTab = bridge.selectTab as unknown as ReturnType<typeof vi.fn>;
    const setTerminalSettings = bridge.setTerminalSettings as unknown as ReturnType<typeof vi.fn>;
    const second: TabSummary = { ...tab, id: "tab_2", label: "Claude · two" };
    state = { windowId: "main", activeTabId: tab.id, tabs: [tab, second], closedTabs: [] };
    render(<App />);
    expect(await screen.findAllByRole("tab")).toHaveLength(2);
    await act(async () => {
      fireEvent.keyDown(window, { key: "Tab", ctrlKey: true });
    });
    expect(selectTab).toHaveBeenLastCalledWith("main", "tab_2");
    await act(async () => {
      fireEvent.keyDown(window, { key: "Tab", ctrlKey: true });
    });
    expect(selectTab).toHaveBeenLastCalledWith("main", "tab_1");
    await act(async () => {
      fireEvent.keyDown(window, { key: "+", ctrlKey: true, shiftKey: true });
    });
    expect(setTerminalSettings).toHaveBeenCalledWith(expect.objectContaining({ fontSize: 15 }));
    state = { windowId: "main", activeTabId: null, tabs: [], closedTabs: [] };
  });
});

describe("App recently closed", () => {
  it("lists the window's closed tabs on the start screen and reopens the chosen one", async () => {
    const bridge = await import("./bridge");
    const reopenClosedTab = bridge.reopenClosedTab as unknown as ReturnType<typeof vi.fn>;
    state = {
      windowId: "main",
      activeTabId: null,
      tabs: [],
      closedTabs: [
        { id: "tab_9", label: "Claude · nine", cwd: "/w/nine", kind: "claude" },
        { id: "tab_8", label: "Shell · eight", cwd: "/w/eight", kind: "shell" },
      ],
    };
    render(<App />);
    const list = await screen.findByRole("region", { name: "Recently closed" });
    fireEvent.click(within(list).getByRole("button", { name: "Reopen Shell · eight" }));
    expect(reopenClosedTab).toHaveBeenLastCalledWith("main", "tab_8");
    state = { windowId: "main", activeTabId: null, tabs: [], closedTabs: [] };
  });
});

describe("App tab context menu", () => {
  const three: TabSummary[] = [
    { ...tab, id: "t_a", label: "A" },
    { ...tab, id: "t_b", label: "B" },
    { ...tab, id: "t_c", label: "C" },
  ];

  it("Close others closes at once when nothing runs and asks once when a terminal runs", async () => {
    state = { windowId: "main", activeTabId: "t_a", tabs: three, closedTabs: [] };
    closeTab.mockClear();
    render(<App />);
    fireEvent.contextMenu(await screen.findByRole("tab", { name: "A" }));
    fireEvent.click(screen.getByRole("menuitem", { name: "Close others" }));
    await waitFor(() => expect(closeTab).toHaveBeenCalledTimes(2));
    expect(closeTab.mock.calls.map((call) => (call as unknown[])[1])).toEqual(["t_b", "t_c"]);
    cleanup();

    state = { windowId: "main", activeTabId: "t_a", tabs: three, closedTabs: [] };
    closeTab.mockClear();
    running.add("t_c");
    render(<App />);
    fireEvent.contextMenu(await screen.findByRole("tab", { name: "A" }));
    fireEvent.click(screen.getByRole("menuitem", { name: "Close to the right" }));
    expect(await screen.findByText("Close tabs?")).toBeTruthy();
    expect(closeTab).not.toHaveBeenCalled();
    expect(screen.getByText(/1 of these tabs still run a terminal/)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Close 2 tabs" }));
    await waitFor(() => expect(closeTab).toHaveBeenCalledTimes(2));
    running.clear();
    state = { windowId: "main", activeTabId: null, tabs: [], closedTabs: [] };
  });

  it("Duplicate opens a tab of the same kind in the same folder", async () => {
    state = { windowId: "main", activeTabId: "t_a", tabs: [{ ...tab, id: "t_a", label: "A", cwd: "/w/dup", kind: "shell" }], closedTabs: [] };
    newTab.mockClear();
    render(<App />);
    fireEvent.contextMenu(await screen.findByRole("tab", { name: "A" }));
    fireEvent.click(screen.getByRole("menuitem", { name: "Duplicate in same folder" }));
    await waitFor(() => expect(newTab).toHaveBeenCalledWith("main", "/w/dup", "shell"));
    state = { windowId: "main", activeTabId: null, tabs: [], closedTabs: [] };
  });
});

describe("App split panes", () => {
  const splitTab: TabSummary = {
    ...tab,
    id: "t_s",
    label: "Claude · split",
    cwd: "/w/split",
    split: { direction: "right", ratio: 0.5, id: "t_s:p2", cwd: "/w/split", folderStatus: "ok" },
  };

  it("Ctrl+Shift+D splits the active tab right and Ctrl+Shift+Alt+D down", async () => {
    const bridge = await import("./bridge");
    const split = bridge.splitTab as unknown as ReturnType<typeof vi.fn>;
    split.mockClear();
    state = { windowId: "main", activeTabId: "tab_1", tabs: [tab], closedTabs: [] };
    render(<App />);
    await screen.findByTestId("terminal");
    await act(async () => {
      fireEvent.keyDown(window, { key: "D", ctrlKey: true, shiftKey: true });
    });
    expect(split).toHaveBeenLastCalledWith("main", "tab_1", "right");
    await act(async () => {
      fireEvent.keyDown(window, { key: "D", ctrlKey: true, shiftKey: true, altKey: true });
    });
    expect(split).toHaveBeenLastCalledWith("main", "tab_1", "down");
    state = { windowId: "main", activeTabId: null, tabs: [], closedTabs: [] };
  });

  it("a split tab shows both panes; Cmd+W closes the focused pane, not the tab", async () => {
    const bridge = await import("./bridge");
    const closePane = bridge.closePane as unknown as ReturnType<typeof vi.fn>;
    const closeTabMock = closeTab;
    closePane.mockClear();
    closeTabMock.mockClear();
    state = { windowId: "main", activeTabId: "t_s", tabs: [splitTab], closedTabs: [] };
    render(<App />);
    expect(await screen.findByTestId("split")).toBeTruthy();
    expect(screen.getAllByTestId("terminal")).toHaveLength(2);
    // Move to the second pane (the arrow toward the second pane), then close.
    await act(async () => {
      fireEvent.keyDown(window, { key: "ArrowRight", ctrlKey: true, shiftKey: true, altKey: true });
    });
    await act(async () => {
      fireEvent.keyDown(window, { key: "W", ctrlKey: true, shiftKey: true });
    });
    expect(closePane).toHaveBeenLastCalledWith("main", "t_s", 2);
    expect(closeTabMock).not.toHaveBeenCalled();
    // Back in the first pane, the same key closes pane 1.
    await act(async () => {
      fireEvent.keyDown(window, { key: "ArrowLeft", ctrlKey: true, shiftKey: true, altKey: true });
    });
    await act(async () => {
      fireEvent.keyDown(window, { key: "W", ctrlKey: true, shiftKey: true });
    });
    expect(closePane).toHaveBeenLastCalledWith("main", "t_s", 1);
    state = { windowId: "main", activeTabId: null, tabs: [], closedTabs: [] };
  });

  it("a one-pane tab: Cmd+W closes the tab", async () => {
    const closeTabMock = closeTab;
    closeTabMock.mockClear();
    state = { windowId: "main", activeTabId: "tab_1", tabs: [tab], closedTabs: [] };
    render(<App />);
    await screen.findByTestId("terminal");
    await act(async () => {
      fireEvent.keyDown(window, { key: "W", ctrlKey: true, shiftKey: true });
    });
    await waitFor(() => expect(closeTabMock).toHaveBeenCalledWith("main", "tab_1"));
  });

  it("a rejected attention summary lands in the recent errors once", async () => {
    clearUiErrors();
    vi.mocked(attentionSummary).mockRejectedValue(new Error("summary down"));
    state = { windowId: "main", activeTabId: null, tabs: [], closedTabs: [] };
    render(<App />);
    await waitFor(() => expect(uiErrors().length).toBe(1));
    expect(uiErrors()[0].source).toBe("Attention summary");
    expect(await screen.findByTestId("error-count")).toBeTruthy();
    vi.mocked(attentionSummary).mockResolvedValue({ needsYou: 0, activeRuns: 0 });
  });

  it("Ctrl+Shift+K opens the command palette over the window, a second press closes it, and a tab row selects", async () => {
    state = { windowId: "main", activeTabId: "tab_1", tabs: [tab], closedTabs: [] };
    render(<App />);
    await screen.findByTestId("terminal");
    await act(async () => {
      fireEvent.keyDown(window, { key: "K", ctrlKey: true, shiftKey: true });
    });
    expect(screen.getByRole("dialog", { name: "Command palette" })).toBeTruthy();
    expect(screen.getByRole("option", { name: /Claude · app/ })).toBeTruthy();
    // Roles come from the overview; Settings sections are listed too.
    expect(await screen.findByRole("option", { name: /Start Planner/ })).toBeTruthy();
    expect(screen.getByRole("option", { name: /Settings: Phone push/ })).toBeTruthy();
    await act(async () => {
      fireEvent.keyDown(window, { key: "K", ctrlKey: true, shiftKey: true });
    });
    expect(screen.queryByRole("dialog", { name: "Command palette" })).toBeNull();
    // Open again over a dialog (New window): it still opens.
    await act(async () => {
      fireEvent.keyDown(window, { key: "N", ctrlKey: true, shiftKey: true });
    });
    expect(await screen.findByRole("dialog", { name: "New window" })).toBeTruthy();
    await act(async () => {
      fireEvent.keyDown(window, { key: "K", ctrlKey: true, shiftKey: true });
    });
    expect(screen.getByRole("dialog", { name: "Command palette" })).toBeTruthy();
    state = { windowId: "main", activeTabId: null, tabs: [], closedTabs: [] };
  });
});

describe("App run pages", () => {
  const runningTab: TabSummary = { ...tab, id: "tab_run", label: "Eagle-Eye · app", runDir: "/d/runs/r-1", role: "eagle-eye" };
  const reading = {
    windowLabel: "main",
    tabId: "tab_run",
    label: "Eagle-Eye · app",
    activity: "working",
    runId: "r-1",
    runDir: "/d/runs/r-1",
    stage: "building",
    waitingFor: null,
    stalled: false,
    prsDone: 1,
    prsTotal: 2,
    prs: [{ id: "PR1", state: "opened", appCheck: null, ci: null, prState: null }],
  };
  const finishedRun = (id: string, day: number, usd: number) => ({
    runId: id,
    runDir: `/d/runs/${id}`,
    folder: "/w/app",
    createdAt: `2026-10-${day}T10:00:00Z`,
    stage: "done",
    updatedAt: null,
    title: id,
    repos: ["app"],
    record: {
      outcome: "done",
      totalTokens: 1,
      estimateUsd: usd,
      minutes: 10 + day,
      waitMinutes: 5,
      spanMinutes: 20,
      prUrls: [],
      firstActivity: null,
      lastActivity: null,
    },
  });

  async function openApp() {
    const bridge = await import("./bridge");
    vi.mocked(bridge.runTabs).mockResolvedValue([reading]);
    vi.mocked(attentionSummary).mockResolvedValue({ needsYou: 0, activeRuns: 1 });
    const api = await import("./run/api");
    vi.mocked(api.runOverview).mockResolvedValue({
      pluginDir: "/app/plugin",
      pluginVersion: "abc123",
      roles: [],
      runs: [finishedRun("r-a", 20, 1.5), finishedRun("r-b", 21, 2.5)],
    } as never);
    state = { windowId: "main", activeTabId: "tab_run", tabs: [runningTab], closedTabs: [] };
    terminalLife.mounts = 0;
    terminalLife.unmounts = 0;
    render(<App />);
    await screen.findByTestId("terminal");
  }

  async function paletteOpen(label: RegExp) {
    await act(async () => {
      fireEvent.keyDown(window, { key: "K", ctrlKey: true, shiftKey: true });
    });
    fireEvent.click(await screen.findByRole("option", { name: label }));
  }

  const endState = () => {
    state = { windowId: "main", activeTabId: null, tabs: [], closedTabs: [] };
  };

  it("shows the dashboard over the terminal; Escape, Close and a tab click close it; no terminal is unmounted", async () => {
    await openApp();
    await waitFor(() => expect(screen.getByTestId("tab-progress-tab_run")).toBeTruthy());
    expect(terminalLife.mounts).toBe(1);

    await paletteOpen(/Run dashboard/);
    expect(await screen.findByRole("region", { name: "Run dashboard" })).toBeTruthy();
    expect(screen.getByTestId("card-tab_run")).toBeTruthy();
    expect(screen.getByTestId("terminal")).toBeTruthy();

    await act(async () => {
      fireEvent.keyDown(window, { key: "Escape" });
    });
    expect(screen.queryByRole("region", { name: "Run dashboard" })).toBeNull();

    await paletteOpen(/Run dashboard/);
    await screen.findByRole("region", { name: "Run dashboard" });
    fireEvent.click(screen.getByRole("tab", { name: /Eagle-Eye · app/ }));
    expect(screen.queryByRole("region", { name: "Run dashboard" })).toBeNull();

    await paletteOpen(/Run dashboard/);
    await screen.findByRole("region", { name: "Run dashboard" });
    fireEvent.click(within(screen.getByRole("region", { name: "Run dashboard" })).getByRole("button", { name: "Close" }));
    expect(screen.queryByRole("region", { name: "Run dashboard" })).toBeNull();

    expect(screen.getByTestId("terminal")).toBeTruthy();
    expect(terminalLife.mounts).toBe(1);
    expect(terminalLife.unmounts).toBe(0);
    vi.mocked(attentionSummary).mockResolvedValue({ needsYou: 0, activeRuns: 0 });
    cleanup();
    endState();
  });

  it("Escape pressed the moment the dashboard appears closes it (no wait for effects or timers)", async () => {
    await openApp();
    await waitFor(() => expect(screen.getByTestId("tab-progress-tab_run")).toBeTruthy());

    // The observer callback runs as a microtask right after React inserts the region: before any macrotask,
    // passive effect or timer, which is how a loaded machine can order them.
    let pressed = false;
    const observer = new MutationObserver(() => {
      if (pressed || !document.querySelector('[aria-label="Run dashboard"]')) return;
      pressed = true;
      fireEvent.keyDown(window, { key: "Escape" });
    });
    observer.observe(document.body, { childList: true, subtree: true });
    try {
      await paletteOpen(/Run dashboard/);
      await waitFor(() => expect(pressed).toBe(true));
      await waitFor(() => expect(screen.queryByRole("region", { name: "Run dashboard" })).toBeNull());
    } finally {
      observer.disconnect();
    }
    cleanup();
    endState();
  });

  it("opens the trace from the palette over the run panel; Escape closes it and no terminal is unmounted", async () => {
    await openApp();
    await waitFor(() => expect(screen.getByTestId("tab-progress-tab_run")).toBeTruthy());
    const { flightSession } = await import("./flight/api");

    await paletteOpen(/Open trace/);
    expect(await screen.findByRole("region", { name: "Trace" })).toBeTruthy();
    expect(vi.mocked(flightSession)).toHaveBeenCalledWith(expect.objectContaining({ runDir: "/d/runs/r-1" }));
    expect(screen.getByTestId("terminal")).toBeTruthy();

    await act(async () => {
      fireEvent.keyDown(window, { key: "Escape" });
    });
    expect(screen.queryByRole("region", { name: "Trace" })).toBeNull();
    expect(screen.getByTestId("terminal")).toBeTruthy();
    expect(terminalLife.mounts).toBe(1);
    expect(terminalLife.unmounts).toBe(0);
    vi.mocked(attentionSummary).mockResolvedValue({ needsYou: 0, activeRuns: 0 });
    cleanup();
    endState();
  });

  it("opens Search traces from the palette; a result opens the Trace on its first match; Escape returns to the same results", async () => {
    await openApp();
    await waitFor(() => expect(screen.getByTestId("tab-progress-tab_run")).toBeTruthy());
    const { flightSession, flightSearch } = await import("./flight/api");
    vi.mocked(flightSearch).mockResolvedValue({
      runs: [
        { runId: "r-1", runDir: "/d/runs/r-1", title: "Add multiply()", outcome: "done", matches: 2, firstEventId: "ev_first" },
      ],
      excludedOff: 0,
      progress: { current: 1, total: 1 },
    });

    await paletteOpen(/Search traces/);
    const search = await screen.findByRole("region", { name: "Trace search" });
    expect(screen.getByTestId("terminal")).toBeTruthy();
    fireEvent.change(within(search).getByLabelText("Name contains"), { target: { value: "cargo test" } });
    fireEvent.submit(within(search).getByRole("form", { name: "Trace filters" }));
    const result = await within(search).findByRole("option", { name: /Add multiply\(\)/ });
    fireEvent.click(result);

    expect(await screen.findByRole("region", { name: "Trace" })).toBeTruthy();
    expect(screen.queryByRole("region", { name: "Trace search" })).toBeNull();
    expect(vi.mocked(flightSession)).toHaveBeenLastCalledWith(
      expect.objectContaining({ runDir: "/d/runs/r-1", focus: "ev_first" }),
    );

    await act(async () => {
      fireEvent.keyDown(window, { key: "Escape" });
    });
    expect(screen.queryByRole("region", { name: "Trace" })).toBeNull();
    const again = await screen.findByRole("region", { name: "Trace search" });
    expect((within(again).getByLabelText("Name contains") as HTMLInputElement).value).toBe("cargo test");
    expect(within(within(again).getByRole("listbox", { name: "Matching runs" })).getAllByRole("option")).toHaveLength(1);
    expect(vi.mocked(flightSearch)).toHaveBeenCalledTimes(1);

    // The header's Search button on a Trace opens the page too; Escape closes the page.
    fireEvent.click(within(again).getByRole("option", { name: /Add multiply\(\)/ }));
    await screen.findByRole("region", { name: "Trace" });
    fireEvent.click(screen.getByRole("button", { name: "Search" }));
    await screen.findByRole("region", { name: "Trace search" });
    await act(async () => {
      fireEvent.keyDown(window, { key: "Escape" });
    });
    expect(screen.queryByRole("region", { name: "Trace search" })).toBeNull();
    expect(screen.getByTestId("terminal")).toBeTruthy();
    expect(terminalLife.mounts).toBe(1);
    expect(terminalLife.unmounts).toBe(0);
    vi.mocked(attentionSummary).mockResolvedValue({ needsYou: 0, activeRuns: 0 });
    cleanup();
    endState();
  });

  it("opens Compare runs from the palette on the folder's two newest finished runs; a value opens its Trace and Escape returns", async () => {
    await openApp();
    await waitFor(() => expect(screen.getByTestId("tab-progress-tab_run")).toBeTruthy());
    const { flightCompare, flightSession } = await import("./flight/api");
    const side = (id: string) => ({ runId: id, runDir: `/d/runs/${id}`, title: id, outcome: "done", indexed: true, off: false });
    vi.mocked(flightCompare).mockResolvedValue({
      a: side("r-a"),
      b: side("r-b"),
      rows: [{ group: "failures", key: "tool.blocked:run-file-write", a: 0, b: 1, differs: true, bEvent: "ev_block" }],
      divergence: 0,
      progress: { current: 2, total: 2 },
    });

    await paletteOpen(/Compare runs/);
    const compare = await screen.findByRole("region", { name: "Compare runs" });
    expect(screen.getByTestId("terminal")).toBeTruthy();
    await waitFor(() => expect(vi.mocked(flightCompare)).toHaveBeenCalledWith("/d/runs/r-a", "/d/runs/r-b"));
    fireEvent.click(await within(compare).findByRole("button", { name: /Open the trace of r-b/ }));
    expect(await screen.findByRole("region", { name: "Trace" })).toBeTruthy();
    expect(vi.mocked(flightSession)).toHaveBeenLastCalledWith(
      expect.objectContaining({ runDir: "/d/runs/r-b", focus: "ev_block" }),
    );
    // Escape returns to Compare with the same two runs.
    await act(async () => {
      fireEvent.keyDown(window, { key: "Escape" });
    });
    expect(screen.queryByRole("region", { name: "Trace" })).toBeNull();
    const again = await screen.findByRole("region", { name: "Compare runs" });
    expect((within(again).getByLabelText("Run A") as HTMLSelectElement).value).toBe("/d/runs/r-a");
    expect((within(again).getByLabelText("Run B") as HTMLSelectElement).value).toBe("/d/runs/r-b");
    // Escape on the page closes it; the terminal was never unmounted.
    await act(async () => {
      fireEvent.keyDown(window, { key: "Escape" });
    });
    expect(screen.queryByRole("region", { name: "Compare runs" })).toBeNull();
    expect(terminalLife.mounts).toBe(1);
    expect(terminalLife.unmounts).toBe(0);
    vi.mocked(attentionSummary).mockResolvedValue({ needsYou: 0, activeRuns: 0 });
    cleanup();
    endState();
  });

  it("offers Resume in the palette for an unfinished run but not for a finished eval run", async () => {
    await openApp();
    await waitFor(() => expect(screen.getByTestId("tab-progress-tab_run")).toBeTruthy());
    const api = await import("./run/api");
    const open = (id: string, title: string, over: object) => ({ ...finishedRun(id, 20, 1), title, ...over });
    vi.mocked(api.runOverview).mockResolvedValue({
      pluginDir: "/app/plugin",
      pluginVersion: "abc123",
      roles: [],
      runs: [
        open("r-wait", "Waits for ship", { stage: "ship-approval", record: null }),
        open("r-eval", "Eval finished", {
          stage: "ship-approval",
          record: null,
          eval: { group: "b-1", index: 1, of: 2, source: "r-0" },
        }),
      ],
    } as never);
    await act(async () => {
      fireEvent.keyDown(window, { key: "K", ctrlKey: true, shiftKey: true });
    });
    await screen.findAllByRole("option");
    await waitFor(() => {
      const texts = screen.getAllByRole("option").map((o) => o.textContent ?? "");
      expect(texts.some((t) => t.includes("Resume: Waits for ship"))).toBe(true);
    });
    const all = screen.getAllByRole("option").map((o) => o.textContent ?? "");
    expect(all.some((t) => t.includes("Resume: Eval finished"))).toBe(false);
    const waiting = all.find((t) => t.includes("Resume: Waits for ship"))!;
    expect(waiting).toContain("Waiting for your go to open the PRs");
    await act(async () => {
      fireEvent.keyDown(window, { key: "Escape" });
    });
    vi.mocked(attentionSummary).mockResolvedValue({ needsYou: 0, activeRuns: 0 });
    cleanup();
    endState();
  });

  it("opens analytics from the palette and from the Runs dialog; terminals stay; Escape closes it", async () => {
    await openApp();

    await paletteOpen(/Run analytics/);
    const page = await screen.findByRole("region", { name: "Run analytics" });
    expect(await within(page).findAllByRole("img")).toHaveLength(3);
    expect(terminalLife.mounts).toBe(1);
    expect(terminalLife.unmounts).toBe(0);
    await act(async () => {
      fireEvent.keyDown(window, { key: "Escape" });
    });
    expect(screen.queryByRole("region", { name: "Run analytics" })).toBeNull();

    // The Runs dialog (status bar > Active runs > All runs): its Analytics button closes the dialog and opens the page.
    fireEvent.click(await screen.findByTestId("status-attention"));
    fireEvent.click(await screen.findByRole("button", { name: "All runs" }));
    const analytics = await screen.findByRole("button", { name: "Analytics" });
    fireEvent.click(analytics);
    expect(await screen.findByRole("region", { name: "Run analytics" })).toBeTruthy();
    expect(screen.queryByRole("dialog", { name: "Runs" })).toBeNull();
    expect(await screen.findAllByRole("img")).toHaveLength(3);
    expect(terminalLife.mounts).toBe(1);
    expect(terminalLife.unmounts).toBe(0);
    await act(async () => {
      fireEvent.keyDown(window, { key: "Escape" });
    });
    expect(screen.queryByRole("region", { name: "Run analytics" })).toBeNull();
    vi.mocked(attentionSummary).mockResolvedValue({ needsYou: 0, activeRuns: 0 });
    cleanup();
    endState();
  });
});

describe("App background starts", () => {
  const pending: TabSummary = { ...runTab, id: "tab_bg", label: "Eagle-Eye · queued", pendingStart: true };
  const opened = () => openLive.mock.calls.map((call) => (call as unknown as [string])[0]);

  const setup = (tabs: TabSummary[]) => {
    cleanup();
    openLive.mockClear();
    windowEvents.clear();
    state = { windowId: "main", activeTabId: tab.id, tabs, closedTabs: [] };
  };
  const selectedLabel = () => screen.getByRole("tab", { selected: true }).textContent ?? "";

  it("tabs-changed reloads, shows the new tab and opens its terminal without selecting it", async () => {
    setup([tab]);
    render(<App />);
    await screen.findByTestId("terminal");
    await waitFor(() => expect(windowEvents.has("tabs-changed")).toBe(true));
    expect(opened()).toEqual([]);
    state = { ...state, tabs: [tab, pending] };
    await act(async () => {
      windowEvents.get("tabs-changed")!({ payload: pending.id });
    });
    expect(await screen.findByRole("tab", { name: /Eagle-Eye · queued/ })).toBeTruthy();
    await waitFor(() => expect(opened()).toEqual(["tab_bg"]));
    expect(selectedLabel()).toContain("Claude · app");
  });

  it("a pending tab present at the first load opens with no event, and the active tab stays", async () => {
    setup([tab, pending]);
    render(<App />);
    await screen.findByTestId("terminal");
    await waitFor(() => expect(opened()).toEqual(["tab_bg"]));
    expect(selectedLabel()).toContain("Claude · app");
  });

  it("a window focus or visibility load opens a tab whose event was missed", async () => {
    setup([tab]);
    render(<App />);
    await screen.findByTestId("terminal");
    state = { ...state, tabs: [tab, pending] };
    await act(async () => {
      window.dispatchEvent(new Event("focus"));
    });
    await waitFor(() => expect(opened()).toEqual(["tab_bg"]));
    expect(selectedLabel()).toContain("Claude · app");

    const second: TabSummary = { ...pending, id: "tab_bg2", label: "Eagle-Eye · wave" };
    state = { ...state, tabs: [tab, pending, second] };
    await act(async () => {
      document.dispatchEvent(new Event("visibilitychange"));
    });
    await waitFor(() => expect(opened()).toContain("tab_bg2"));
    expect(selectedLabel()).toContain("Claude · app");
  });

  it("a restored tab (not pending) is never opened by a load", async () => {
    setup([tab, { ...runTab, id: "tab_restored", pendingStart: false }]);
    render(<App />);
    await screen.findByTestId("terminal");
    await act(async () => {
      window.dispatchEvent(new Event("focus"));
    });
    expect(opened()).toEqual([]);
  });

  it("no background open while the window's account is missing", async () => {
    setup([tab, pending]);
    context = { ...personal, accountMissing: true };
    try {
      render(<App />);
      await screen.findByTestId("status-account");
      await act(async () => {
        window.dispatchEvent(new Event("focus"));
      });
      expect(opened()).toEqual([]);
    } finally {
      context = personal;
    }
  });
});

describe("App trace links", () => {
  const RUN = "/d/runs/r-20261105-230341-7b3c";
  const LINK = "dcterminalv2://run/r-20261105-230341-7b3c";

  const reset = async () => {
    const api = await import("./run/api");
    vi.mocked(api.takeRunLink).mockReset().mockResolvedValue(null);
    vi.mocked(api.resolveRunLink).mockReset();
    state = { windowId: "main", activeTabId: "tab_1", tabs: [tab], closedTabs: [] };
    return api;
  };
  const openPalette = async () => {
    await act(async () => {
      fireEvent.keyDown(window, { key: "K", ctrlKey: true, shiftKey: true });
    });
    return screen.getByRole("combobox");
  };
  const type = (box: HTMLElement, text: string) => fireEvent.change(box, { target: { value: text } });
  const traceOpenedFor = async (runDir: string) => {
    const { flightSession } = await import("./flight/api");
    expect(await screen.findByRole("region", { name: "Trace" })).toBeTruthy();
    expect(vi.mocked(flightSession)).toHaveBeenCalledWith(expect.objectContaining({ runDir }));
  };
  const finish = () => {
    cleanup();
    state = { windowId: "main", activeTabId: null, tabs: [], closedTabs: [] };
  };

  it("a link waiting on load opens its Trace, once", async () => {
    const api = await reset();
    vi.mocked(api.takeRunLink).mockResolvedValueOnce({ runDir: RUN });
    render(<App />);
    await traceOpenedFor(RUN);
    expect(api.takeRunLink).toHaveBeenCalledTimes(1);
    finish();
  });

  it("a run-link event pulls the link, and an empty slot opens nothing", async () => {
    const api = await reset();
    render(<App />);
    await screen.findByTestId("terminal");
    await waitFor(() => expect(windowEvents.has("run-link")).toBe(true));
    await act(async () => {
      windowEvents.get("run-link")!({ payload: null });
    });
    expect(api.takeRunLink).toHaveBeenCalledTimes(2);
    expect(screen.queryByRole("region", { name: "Trace" })).toBeNull();
    vi.mocked(api.takeRunLink).mockResolvedValueOnce({ runDir: RUN });
    await act(async () => {
      windowEvents.get("run-link")!({ payload: null });
    });
    await traceOpenedFor(RUN);
    finish();
  });

  it("a refused link shows the message and opens no Trace", async () => {
    const api = await reset();
    vi.mocked(api.takeRunLink).mockResolvedValueOnce({ error: "Run r-20261105-230341-7b3c is not on this Mac" });
    render(<App />);
    expect(await screen.findByText(/Run r-20261105-230341-7b3c is not on this Mac/)).toBeTruthy();
    expect(screen.queryByRole("region", { name: "Trace" })).toBeNull();
    finish();
  });

  it("typing a valid link into the palette lists Open run link…, and choosing it opens the Trace", async () => {
    const api = await reset();
    vi.mocked(api.resolveRunLink).mockResolvedValue({ runDir: RUN });
    const prompt = vi.spyOn(window, "prompt");
    render(<App />);
    await screen.findByTestId("terminal");
    type(await openPalette(), LINK);
    const row = await screen.findByRole("option", { name: /Open run link…/ });
    expect(screen.getAllByRole("option")[0]).toBe(row);
    fireEvent.click(row);
    await traceOpenedFor(RUN);
    expect(api.resolveRunLink).toHaveBeenCalledWith(LINK);
    expect(prompt).not.toHaveBeenCalled();
    prompt.mockRestore();
    finish();
  });

  it("a malformed link-like query lists the item, and choosing it shows the message", async () => {
    const api = await reset();
    vi.mocked(api.resolveRunLink).mockResolvedValue({ error: "That run is not on this Mac" });
    render(<App />);
    await screen.findByTestId("terminal");
    type(await openPalette(), "r-123");
    fireEvent.click(await screen.findByRole("option", { name: /Open run link…/ }));
    expect(await screen.findByText(/That run is not on this Mac/)).toBeTruthy();
    expect(screen.queryByRole("region", { name: "Trace" })).toBeNull();
    finish();
  });

  it("an ordinary query lists no link item", async () => {
    await reset();
    render(<App />);
    await screen.findByTestId("terminal");
    type(await openPalette(), "settings");
    await screen.findByRole("option", { name: /Settings: Phone push/ });
    expect(screen.queryByRole("option", { name: /Open run link…/ })).toBeNull();
    finish();
  });
});
