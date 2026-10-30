import { useCallback, useEffect, useRef, useState } from "react";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import {
  closePane,
  closeTab,
  getAppState,
  attentionSummary,
  getClaudeUsage,
  getSettings,
  moveTab,
  newTab,
  openAccountWindow,
  renameTab,
  reopenClosedTab,
  selectTab,
  setSplitRatio,
  setTerminalSettings,
  setWindowAccount,
  splitTab,
  windowContext,
  type AppStateSnapshot,
  type AttentionSummary,
  type RateWindow,
  type SettingsView,
  type TabSummary,
  type WindowContext,
} from "./bridge";
import { ActiveRuns } from "./components/ActiveRuns";
import { AttentionNotices } from "./components/AttentionNotice";
import { AccountPicker } from "./components/AccountPicker";
import { Dialog } from "./components/Dialog";
import { InlineNotice } from "./components/InlineNotice";
import { NewTabForm } from "./components/NewTabForm";
import { RunPanel } from "./components/RunPanel";
import { PreviewHost } from "./components/PreviewPane";
import { RunsView } from "./components/RunsView";
import { StartScreen } from "./components/StartScreen";
import { resumeRun, runOverview, startRun, type RunOverview, type StartedRun } from "./run/api";
import { CommandPalette, type PaletteItem } from "./components/CommandPalette";
import { stageText } from "./run/words";
import { SettingsPage } from "./components/SettingsPage";
import { StatusBar } from "./components/StatusBar";
import { TabBar } from "./components/TabBar";
import { UsagePopover } from "./components/UsagePopover";
import { SplitTerminals } from "./components/SplitTerminals";
import { destroyTerminal, TerminalView } from "./components/TerminalView";
import { newTabDefaults, type NewTabDraft } from "./newTab";
import { detectPlatform, SCRATCHPAD_EVENT } from "./terminal/keys";
import { ACCOUNT_REMOVED, accountProblem } from "./terminal/openPlan";
import { dropLivePty, ptyExitCode, ptyRunning, subscribeLivePty, subscribePtyExit } from "./terminal/live";
import { focusParkedTerminal, refitTerminal, requestTerminalSearch, setCompactPaste, setCopyOnSelect, setOptionAsMeta, setTerminalTheme } from "./terminal/park";
import { applyTheme, watchSystemTheme } from "./theme";
import { reportError, setErrorNotifier } from "./uiErrors";
import { statusLimit, type WindowHistory } from "./usage/limits";
import { keyAction, pageAction, type PageAction } from "./pageActions";
import { zoomedFontSize } from "./keyTable";
import { ClaudeVersionBanner } from "./components/ClaudeVersionBanner";
import { ImportDevDialog, useDevImport } from "./components/ImportDevDialog";
import { observeTabs, pollsActivity } from "./run/observe";
import { RUN_STATE_EVENT, TAB_STATE_EVENT, usePushRefresh } from "./run/events";
import "./App.css";

/** How often the usage line is read again while nothing else asks. */
const USAGE_POLL_MS = 60_000;
/** How often the status bar reads the attention summary (an in-memory read). */
const ATTENTION_POLL_MS = 4_000;

/** The Settings sections (their `aria-label`s) the command palette opens. */
const SETTINGS_SECTIONS = [
  "Claude accounts",
  "ClickUp",
  "Terminal",
  "Runs",
  "Activity",
  "Notifications",
  "Phone push",
  "Appearance",
];

/** This window's label: the id every window-scoped command names. */
function currentWindowId(): string {
  try {
    return getCurrentWebviewWindow().label;
  } catch {
    // Not inside Tauri (a plain browser).
    return "main";
  }
}

/** Settings that act in the page: terminal options and the theme. */
function applySettings(view: SettingsView): void {
  setOptionAsMeta(view.terminal.optionAsMeta);
  setCopyOnSelect(view.terminal.copyOnSelect);
  setCompactPaste(view.terminal.compactPaste ?? true);
  applyTheme(view.ui.theme);
  setTerminalTheme(view.ui.theme);
}

function App() {
  const windowId = useRef(currentWindowId()).current;
  const [ctx, setCtx] = useState<WindowContext | null>(null);
  const [settings, setSettings] = useState<SettingsView | null>(null);
  const [state, setState] = useState<AppStateSnapshot | null>(null);
  const [usage, setUsage] = useState<RateWindow[] | null>(null);
  const [usageHistory, setUsageHistory] = useState<WindowHistory[]>([]);
  const [message, setMessage] = useState<string | null>(null);
  // Tabs that need you and unfinished runs, in every window (the status bar's control).
  const [attention, setAttention] = useState<AttentionSummary | null>(null);
  const [activeRunsOpen, setActiveRunsOpen] = useState(false);
  const [dialog, setDialog] = useState<"new-tab" | "new-window" | "pick-account" | "settings" | "runs" | null>(null);
  const [confirmClose, setConfirmClose] = useState<string | null>(null);
  // The command palette (Cmd+K), the run list it offers, and the Settings section it opens.
  const [paletteOpen, setPaletteOpen] = useState(false);
  const [paletteRuns, setPaletteRuns] = useState<RunOverview | null>(null);
  const [settingsSection, setSettingsSection] = useState<string | undefined>(undefined);
  const [lastFolder, setLastFolder] = useState("");
  const devImport = useDevImport(windowId);
  const devImportRef = useRef(devImport);
  devImportRef.current = devImport;
  // The new-tab screen shows the plain tab form instead of New run.
  const [plainTab, setPlainTab] = useState(false);
  // Re-render when a terminal starts or exits (the tab bar dims exited tabs).
  const [, setLiveVersion] = useState(0);

  const fail = useCallback((source: string, err: unknown) => {
    reportError(source, err, { notify: true });
  }, []);

  useEffect(() => {
    setErrorNotifier((title, text) => setMessage(`${title}: ${text}`));
    return () => setErrorNotifier(null);
  }, []);

  useEffect(() => {
    windowContext().then(setCtx, (err: unknown) => fail("Window", err));
    getSettings().then(
      (view) => {
        setSettings(view);
        applySettings(view);
      },
      (err: unknown) => fail("Settings", err),
    );
    getAppState(windowId).then(setState, (err: unknown) => fail("Tabs", err));
  }, [fail, windowId]);

  // "Match system" follows the OS while it is chosen.
  useEffect(() => {
    if (settings?.ui.theme !== "system") return;
    return watchSystemTheme(() => {
      applyTheme("system");
      setTerminalTheme("system");
    });
  }, [settings?.ui.theme]);

  const refreshUsage = useCallback(() => {
    getClaudeUsage(windowId).then(
      (snapshot) => {
        setUsage(snapshot.windows);
        setUsageHistory(snapshot.history ?? []);
      },
      (err: unknown) => reportError("Claude usage", err, { once: true }),
    );
  }, [windowId]);

  useEffect(() => {
    refreshUsage();
    const timer = window.setInterval(refreshUsage, USAGE_POLL_MS);
    window.addEventListener("focus", refreshUsage);
    return () => {
      window.clearInterval(timer);
      window.removeEventListener("focus", refreshUsage);
    };
  }, [refreshUsage]);

  const refreshAttention = useCallback(() => {
    attentionSummary().then(setAttention, (err: unknown) =>
      reportError("Attention summary", err, { once: "attention-summary" }),
    );
  }, []);

  // The detector runs in the backend whatever this window does; the summary
  // is read now, when the window gets focus, and when the detector announces
  // something (a push event or an attention notice); it polls every few
  // seconds only while no event arrives.
  usePushRefresh(["attention", TAB_STATE_EVENT, RUN_STATE_EVENT], refreshAttention, {
    fastMs: ATTENTION_POLL_MS,
  });

  useEffect(() => subscribeLivePty(() => setLiveVersion((n) => n + 1)), []);
  useEffect(() => subscribePtyExit(() => refreshUsage()), [refreshUsage]);

  const active = state?.tabs.find((tab) => tab.id === state.activeTabId) ?? null;

  const select = (tabId: string) => {
    setState((current) => (current ? { ...current, activeTabId: tabId } : current));
    selectTab(windowId, tabId).catch((err: unknown) => fail("Select tab", err));
  };

  const reload = () => getAppState(windowId).then(setState);
  const reloadRef = useRef(reload);
  reloadRef.current = reload;

  const create = async (draft: NewTabDraft) => {
    await newTab(windowId, draft.cwd, draft.kind);
    setLastFolder(draft.cwd);
    setDialog(null);
    await reload();
  };

  const started = async (run: StartedRun) => {
    setLastFolder(run.tab.cwd);
    setDialog(null);
    setPlainTab(false);
    await reload();
  };

  // P3b: what each tab's Claude does, from its hook events.
  const [activity, setActivity] = useState<Record<string, string>>({});
  const [usageTab, setUsageTab] = useState<string | null>(null);
  // Any Claude tab has hook state while "Track activity in every Claude tab" is on.
  const hasClaudeTab = pollsActivity(state?.tabs ?? []);
  const refreshActivity = useCallback(
    () =>
      observeTabs().then(
        (states) => setActivity(Object.fromEntries(Object.entries(states).map(([id, s]) => [id, s.activity]))),
        (err: unknown) => reportError("Tab activity", err, { once: "observe-tabs" }),
      ),
    [],
  );
  usePushRefresh(TAB_STATE_EVENT, () => (hasClaudeTab ? refreshActivity() : undefined), { fastMs: 3000 });

  const resumeActiveRun = (runDir: string) => {
    resumeRun(windowId, runDir).then(
      (run) => void started(run),
      (err: unknown) => fail("Resume", err),
    );
  };

  // A closed tab's terminals go: its own and its second pane's.
  const releaseTerminals = (tabId: string) => {
    const split = state?.tabs.find((tab) => tab.id === tabId)?.split;
    for (const id of split ? [tabId, split.id] : [tabId]) {
      destroyTerminal(id);
      dropLivePty(id);
    }
  };

  const tabRuns = (tabId: string) => {
    const split = state?.tabs.find((tab) => tab.id === tabId)?.split;
    return ptyRunning(tabId) || (split ? ptyRunning(split.id) : false);
  };

  const doClose = (tabId: string) => {
    setConfirmClose(null);
    closeTab(windowId, tabId).then(
      (next) => {
        releaseTerminals(tabId);
        setState(next);
      },
      (err: unknown) => fail("Close tab", err),
    );
  };

  // A running terminal asks first; an exited one closes at once.
  const requestClose = (tabId: string) => {
    if (tabRuns(tabId)) setConfirmClose(tabId);
    else doClose(tabId);
  };

  // ---- split panes -------------------------------------------------------
  // The pane the keyboard is in, per tab (1 is the tab's own terminal), and
  // a counter that remounts a tab's terminals when its first pane closes.
  const [focusedPanes, setFocusedPanes] = useState<Record<string, 1 | 2>>({});
  const [epochs, setEpochs] = useState<Record<string, number>>({});
  const [confirmPane, setConfirmPane] = useState<{ tabId: string; pane: 1 | 2 } | null>(null);
  const paneOf = (tabId: string): 1 | 2 => focusedPanes[tabId] ?? 1;
  const focusPane = (tabId: string, pane: 1 | 2) => {
    setFocusedPanes((current) => (current[tabId] === pane ? current : { ...current, [tabId]: pane }));
    const split = state?.tabs.find((tab) => tab.id === tabId)?.split;
    focusParkedTerminal(pane === 2 && split ? split.id : tabId);
  };

  const splitActive = (tabId: string, direction: "right" | "down") => {
    splitTab(windowId, tabId, direction).then(
      () => {
        setFocusedPanes((current) => ({ ...current, [tabId]: 2 }));
        return reload();
      },
      (err: unknown) => fail("Split tab", err),
    );
  };

  const closePaneNow = (tabId: string, pane: 1 | 2) => {
    setConfirmPane(null);
    const tab = state?.tabs.find((item) => item.id === tabId);
    if (!tab?.split) return;
    const gone = pane === 2 ? [tab.split.id] : [tab.id, tab.split.id];
    for (const id of gone) {
      destroyTerminal(id);
      dropLivePty(id);
    }
    closePane(windowId, tabId, pane).then(
      () => {
        setFocusedPanes((current) => ({ ...current, [tabId]: 1 }));
        if (pane === 1) setEpochs((current) => ({ ...current, [tabId]: (current[tabId] ?? 0) + 1 }));
        return reload();
      },
      (err: unknown) => {
        fail("Close pane", err);
        void reload();
      },
    );
  };

  // Cmd+W: the focused pane when the tab has two, else the tab. A run's
  // first pane is the run, so closing it closes the tab.
  const closeFocusedPane = (tab: TabSummary) => {
    if (!tab.split) return requestClose(tab.id);
    const pane = paneOf(tab.id);
    if (pane === 1 && tab.runDir) return requestClose(tab.id);
    const running = ptyRunning(pane === 1 ? tab.id : tab.split.id);
    if (running) setConfirmPane({ tabId: tab.id, pane });
    else closePaneNow(tab.id, pane);
  };

  const terminalsOf = (tab: TabSummary, autoFocus: boolean) => {
    const key = `${tab.id}-${epochs[tab.id] ?? 0}`;
    return tab.split ? (
      <SplitTerminals
        key={key}
        tab={tab}
        fontSize={fontSize}
        autoFocus={autoFocus}
        focusedPane={paneOf(tab.id)}
        onPaneFocus={(pane) => setFocusedPanes((current) => (current[tab.id] === pane ? current : { ...current, [tab.id]: pane }))}
        onFolderChanged={() => void reload()}
        onRatio={(ratio) => {
          setState((current) =>
            current
              ? { ...current, tabs: current.tabs.map((t) => (t.id === tab.id && t.split ? { ...t, split: { ...t.split, ratio } } : t)) }
              : current,
          );
          setSplitRatio(tab.id, ratio).catch((err: unknown) => fail("Resize panes", err));
        }}
      />
    ) : (
      <TerminalView
        key={key}
        tab={tab}
        fontSize={fontSize}
        autoFocus={autoFocus}
        onFolderChanged={() => void reload()}
      />
    );
  };

  const reopen = (tabId?: string) => {
    reopenClosedTab(windowId, tabId ?? null).then(
      () => reload(),
      (err: unknown) => fail("Reopen tab", err),
    );
  };

  const [confirmCloseMany, setConfirmCloseMany] = useState<string[] | null>(null);

  const closeNow = async (ids: string[]) => {
    setConfirmCloseMany(null);
    for (const id of ids) {
      try {
        const next = await closeTab(windowId, id);
        releaseTerminals(id);
        setState(next);
      } catch (err) {
        fail("Close tab", err);
        break;
      }
    }
  };

  // Close others / Close to the right: one question when any terminal runs.
  const requestCloseMany = (ids: string[]) => {
    if (ids.some((id) => tabRuns(id))) setConfirmCloseMany(ids);
    else void closeNow(ids);
  };

  const duplicate = (tabId: string) => {
    const source = state?.tabs.find((tab) => tab.id === tabId);
    if (!source) return;
    newTab(windowId, source.cwd, source.kind).then(
      () => reload(),
      (err: unknown) => fail("Duplicate tab", err),
    );
  };

  const reveal = (tabId: string) => {
    const source = state?.tabs.find((tab) => tab.id === tabId);
    if (!source) return;
    revealItemInDir(source.cwd).catch((err: unknown) => fail("Reveal folder", err));
  };

  const move = (tabId: string, index: number) => {
    moveTab(windowId, tabId, index).then(setState, (err: unknown) => fail("Move tab", err));
  };

  const rename = (tabId: string, label: string) => {
    renameTab(tabId, label).then(
      () => reload(),
      (err: unknown) => fail("Rename tab", err),
    );
  };

  // The latest terminal settings, so quick repeated zoom keys build on each other.
  const terminalRef = useRef(settings?.terminal);
  terminalRef.current = settings?.terminal;
  const zoomFont = (kind: PageAction["kind"]) => {
    const current = terminalRef.current;
    if (!current) return;
    const next = zoomedFontSize(current.fontSize, kind);
    if (next == null || next === current.fontSize) return;
    terminalRef.current = { ...current, fontSize: next };
    setSettings((view) => (view ? { ...view, terminal: { ...view.terminal, fontSize: next } } : view));
    setTerminalSettings({ ...current, fontSize: next }).then(
      (saved) => setSettings((view) => (view ? { ...view, terminal: saved } : view)),
      (err: unknown) => fail("Font size", err),
    );
  };

  const run = (action: PageAction) => {
    switch (action.kind) {
      case "new-tab":
        setPlainTab(false);
        setDialog("new-tab");
        break;
      case "close-tab":
        if (active) closeFocusedPane(active);
        break;
      case "split":
        if (active && !active.split && !blocked) splitActive(active.id, action.direction);
        break;
      case "focus-pane": {
        if (!active?.split) break;
        const horizontal = active.split.direction === "right";
        const towardFirst = horizontal ? action.direction === "left" : action.direction === "up";
        const towardSecond = horizontal ? action.direction === "right" : action.direction === "down";
        if (towardFirst) focusPane(active.id, 1);
        else if (towardSecond) focusPane(active.id, 2);
        break;
      }
      case "reopen-closed-tab":
        reopen();
        break;
      case "reopen-closed":
        reopen(action.tabId);
        break;
      case "settings":
        setDialog("settings");
        break;
      case "new-window":
        setDialog("new-window");
        break;
      case "find":
        if (active) requestTerminalSearch(active.id);
        break;
      case "palette":
        setPaletteOpen((open) => !open);
        break;
      case "select": {
        const tab = state?.tabs[action.index];
        if (tab) select(tab.id);
        break;
      }
      case "next-tab":
      case "prev-tab": {
        const tabs = state?.tabs ?? [];
        const at = tabs.findIndex((tab) => tab.id === state?.activeTabId);
        if (tabs.length > 1 && at >= 0) {
          const step = action.kind === "next-tab" ? 1 : -1;
          select(tabs[(at + step + tabs.length) % tabs.length].id);
        }
        break;
      }
      case "zoom-in":
      case "zoom-out":
      case "zoom-reset":
        zoomFont(action.kind);
        break;
    }
  };
  // The listeners below outlive renders; they call the latest `run`.
  const runRef = useRef(run);
  runRef.current = run;

  // The native menu (macOS) sends its items to the focused window.
  useEffect(() => {
    let disposed = false;
    const unlisten: (() => void)[] = [];
    try {
      const win = getCurrentWebviewWindow();
      const keep = (promise: Promise<() => void>) =>
        promise.then(
          (fn) => (disposed ? fn() : unlisten.push(fn)),
          (err: unknown) => reportError("Menu", err, { once: true }),
        );
      keep(win.listen("new-window", () => runRef.current({ kind: "new-window" })));
      // Resume of a run whose tab is live in this window focuses that tab.
      keep(win.listen<string>("focus-tab", () => void reloadRef.current()));
      keep(
        win.listen<string>("menu", (event) => {
          if (event.payload === "import-dev") {
            devImportRef.current.open();
            return;
          }
          const action = pageAction(event.payload);
          if (action) runRef.current(action);
        }),
      );
    } catch {
      // Not inside Tauri (unit tests, a plain browser).
    }
    return () => {
      disposed = true;
      unlisten.forEach((fn) => fn());
    };
  }, []);

  // Keys the menu does not cover (Find, Cmd+1…9), and every shortcut where
  // there is no native menu (Windows, Linux).
  useEffect(() => {
    const platform = detectPlatform(navigator.platform);
    const onKey = (event: KeyboardEvent) => {
      const action = keyAction(event, platform);
      if (!action) return;
      // The palette opens over any screen, a dialog included, and a second press closes it;
      // every other shortcut waits while a dialog or the palette is open.
      if (action.kind !== "palette" && (dialog || paletteOpen)) return;
      event.preventDefault();
      runRef.current(action);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [dialog, paletteOpen]);

  // The palette offers unfinished runs and the role tiles: read them when it opens.
  useEffect(() => {
    if (!paletteOpen) return;
    runOverview().then(setPaletteRuns, (err: unknown) => reportError("Command palette", err, { once: true }));
  }, [paletteOpen]);

  const paletteItems = (): PaletteItem[] => {
    const items: PaletteItem[] = (state?.tabs ?? []).map((tab) => ({
      id: `tab:${tab.id}`,
      group: "Tabs",
      label: tab.label,
      detail: tab.cwd,
      run: () => select(tab.id),
    }));
    for (const item of paletteRuns?.runs ?? []) {
      if (item.stage === "done" || item.stage === "failed") continue;
      items.push({
        id: `run:${item.runDir}`,
        group: "Runs",
        label: `Resume: ${item.title ?? item.runId}`,
        detail: stageText(item.stage),
        run: () =>
          resumeRun(windowId, item.runDir).then((started_) => {
            void started(started_);
          }),
      });
    }
    for (const role of paletteRuns?.roles ?? []) {
      items.push({
        id: `role:${role.id}`,
        group: "Roles",
        label: `Start ${role.label}`,
        detail: active ? active.cwd : "in the start screen",
        run: () => {
          if (!active) {
            // No tab to take a folder from: the start screen picks it.
            setPlainTab(false);
            setDialog("new-tab");
            return;
          }
          return startRun(windowId, active.cwd, "", role.id).then((run) => started(run));
        },
      });
    }
    for (const section of SETTINGS_SECTIONS) {
      items.push({
        id: `settings:${section}`,
        group: "Settings",
        label: `Settings: ${section}`,
        run: () => {
          setSettingsSection(section);
          setDialog("settings");
        },
      });
    }
    if (active) {
      items.push({
        id: "scratchpad",
        group: "Tabs",
        label: "Toggle prompt scratchpad",
        detail: "Cmd+Shift+E",
        run: () => {
          window.dispatchEvent(new Event(SCRATCHPAD_EVENT));
        },
      });
    }
    return items;
  };

  const accountMissing = ctx?.accountMissing ?? false;
  // A Claude tab of a removed account shows the notice, never a terminal.
  const blocked = active ? accountProblem(active, accountMissing) : null;

  const exited = new Set((state?.tabs ?? []).filter((tab) => ptyExitCode(tab.id) != null).map((tab) => tab.id));
  const fontSize = settings?.terminal.fontSize ?? 14;

  return (
    <main className="workspace">
      {state && state.tabs.length > 0 && (
        <TabBar
          tabs={state.tabs}
          activeTabId={state.activeTabId}
          exited={exited}
          activity={activity}
          onActivityClick={(id) => setUsageTab((current) => (current === id ? null : id))}
          onSelect={select}
          onClose={requestClose}
          onRename={rename}
          onMove={move}
          onDuplicate={duplicate}
          onReveal={reveal}
          onCloseMany={requestCloseMany}
          onSplit={splitActive}
          onNew={() => {
            setPlainTab(false);
            setDialog("new-tab");
          }}
        />
      )}
      {usageTab && <UsagePopover key={usageTab} tabId={usageTab} onClose={() => setUsageTab(null)} />}
      <section className="workspace-body">
        <ClaudeVersionBanner windowId={windowId} />
        {devImport.status && <ImportDevDialog status={devImport.status} onClose={devImport.close} />}
        {accountMissing && !blocked && (
          <InlineNotice
            tone="warning"
            action={
              <button type="button" className="secondary-button" onClick={() => setDialog("pick-account")}>
                Pick an account
              </button>
            }
          >
            {ACCOUNT_REMOVED}
          </InlineNotice>
        )}
        {state && state.tabs.length === 0 && (
          <div className="start-screen">
            <h1>{plainTab ? "New tab" : "New run"}</h1>
            <p className="hint">
              {ctx ? `Claude tabs in this window use ${ctx.accountName} (${ctx.config.display}).` : ""}
            </p>
            {plainTab ? (
              <NewTabForm
                initial={newTabDefaults(state, lastFolder)}
                onCreate={create}
                onCancel={() => setPlainTab(false)}
              />
            ) : (
              <StartScreen
                windowId={windowId}
                initialFolder={newTabDefaults(state, lastFolder).cwd}
                onStarted={(run) => void started(run)}
                onPlainTab={() => setPlainTab(true)}
                onShowAllRuns={() => setDialog("runs")}
                usageWindows={usage}
              />
            )}
            {state.closedTabs.length > 0 && (
              <section className="recently-closed" aria-label="Recently closed">
                <h3>Recently closed</h3>
                <ul>
                  {state.closedTabs.slice(0, 10).map((closed) => (
                    <li key={closed.id}>
                      <button type="button" className="link-button" onClick={() => reopen(closed.id)}>
                        Reopen {closed.label}
                      </button>
                      <span className="hint"> {closed.cwd}</span>
                    </li>
                  ))}
                </ul>
              </section>
            )}
          </div>
        )}
        {active && blocked && (
          <div className="terminal-view terminal-problem">
            <p className="error">{blocked}</p>
            <button type="button" className="primary-button" onClick={() => setDialog("pick-account")}>
              Pick an account
            </button>
          </div>
        )}
        {active && !blocked && !active.runDir && terminalsOf(active, !dialog)}
        {active && !blocked && active.runDir && (
          <div className="run-layout">
            {terminalsOf(active, !dialog)}
            <PreviewHost onLayoutChange={() => refitTerminal(active.id)} />
            <RunPanel
              key={active.runDir}
              runDir={active.runDir}
              tabId={active.id}
              onResume={() => resumeActiveRun(active.runDir!)}
              onGoToPrompt={() => focusParkedTerminal(active.id)}
              windowId={windowId}
              onTabsChanged={() => void reload()}
              onLayoutChange={() => refitTerminal(active.id)}
            />
          </div>
        )}
      </section>
      <AttentionNotices />
      {activeRunsOpen && (
        <ActiveRuns
          onClose={() => setActiveRunsOpen(false)}
          onAllRuns={() => setDialog("runs")}
        />
      )}
      <StatusBar
        account={ctx ? { name: ctx.accountName, config: ctx.config } : null}
        folder={active?.cwd ?? null}
        usage={usage ? statusLimit(usage, Date.now(), usageHistory) : null}
        message={message ? { text: message, onDismiss: () => setMessage(null) } : null}
        attention={attention}
        onAttention={() => setActiveRunsOpen((open) => !open)}
        onNewWindow={() => setDialog("new-window")}
        onSettings={() => setDialog("settings")}
      />
      {dialog === "new-tab" && (
        <Dialog
          title={plainTab ? "New tab" : "New run"}
          onClose={() => setDialog(null)}
          className={plainTab ? "" : "start-dialog"}
        >
          {plainTab ? (
            <NewTabForm
              initial={newTabDefaults(state, lastFolder)}
              onCreate={create}
              onCancel={() => setPlainTab(false)}
            />
          ) : (
            <StartScreen
              windowId={windowId}
              initialFolder={newTabDefaults(state, lastFolder).cwd}
              onStarted={(run) => void started(run)}
              onPlainTab={() => setPlainTab(true)}
                onShowAllRuns={() => setDialog("runs")}
                usageWindows={usage}
            />
          )}
        </Dialog>
      )}
      {dialog === "new-window" && settings && (
        <AccountPicker
          accounts={settings.accounts}
          current={ctx?.accountId ?? ""}
          onOpen={(accountId) => openAccountWindow(accountId).then(() => undefined)}
          onClose={() => setDialog(null)}
        />
      )}
      {dialog === "pick-account" && settings && (
        <AccountPicker
          accounts={settings.accounts}
          current=""
          title="Pick an account"
          submitLabel="Use account"
          hint="This window's account was removed from Settings. Its Claude tabs start under the account you pick."
          onOpen={(accountId) => setWindowAccount(accountId).then(setCtx)}
          onClose={() => setDialog(null)}
        />
      )}
      {dialog === "settings" && settings && (
        <SettingsPage
          view={settings}
          section={settingsSection}
          onSaved={(view) => {
            setSettings(view);
            applySettings(view);
            // An account rename or a new folder retitles this window.
            windowContext().then(setCtx, (err: unknown) => fail("Window", err));
          }}
          onClose={() => {
            setDialog(null);
            setSettingsSection(undefined);
          }}
        />
      )}
      {dialog === "runs" && (
        <RunsView
          windowId={windowId}
          onClose={() => setDialog(null)}
          onStarted={(run) => void started(run)}
        />
      )}
      {paletteOpen && <CommandPalette items={paletteItems()} onClose={() => setPaletteOpen(false)} />}
      {confirmCloseMany && (
        <Dialog
          title="Close tabs?"
          onClose={() => setConfirmCloseMany(null)}
          footer={
            <>
              <button type="button" className="secondary-button" onClick={() => setConfirmCloseMany(null)}>
                Cancel
              </button>
              <button type="button" className="danger-button" onClick={() => void closeNow(confirmCloseMany)}>
                Close {confirmCloseMany.length} tabs
              </button>
            </>
          }
        >
          <p>
            {confirmCloseMany.filter((id) => tabRuns(id)).length} of these tabs still run a terminal. They will be
            stopped. You can reopen the tabs later.
          </p>
        </Dialog>
      )}
      {confirmPane && (
        <Dialog
          title="Close pane?"
          onClose={() => setConfirmPane(null)}
          footer={
            <>
              <button type="button" className="secondary-button" onClick={() => setConfirmPane(null)}>
                Cancel
              </button>
              <button
                type="button"
                className="danger-button"
                onClick={() => closePaneNow(confirmPane.tabId, confirmPane.pane)}
              >
                Close pane
              </button>
            </>
          }
        >
          <p>
            Its terminal is still running and will be stopped.
            {confirmPane.pane === 1 ? " The other pane's shell restarts as the tab." : ""}
          </p>
        </Dialog>
      )}
      {confirmClose && (
        <Dialog
          title="Close tab?"
          onClose={() => setConfirmClose(null)}
          footer={
            <>
              <button type="button" className="secondary-button" onClick={() => setConfirmClose(null)}>
                Cancel
              </button>
              <button type="button" className="danger-button" onClick={() => doClose(confirmClose)}>
                Close tab
              </button>
            </>
          }
        >
          <p>Its terminal is still running and will be stopped. You can reopen the tab later.</p>
        </Dialog>
      )}
    </main>
  );
}

export default App;
