import { useCallback, useEffect, useRef, useState } from "react";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import {
  closeTab,
  getAppState,
  getClaudeUsage,
  getSettings,
  newTab,
  openAccountWindow,
  renameTab,
  reopenClosedTab,
  selectTab,
  windowContext,
  type AppStateSnapshot,
  type RateWindow,
  type SettingsView,
  type WindowContext,
} from "./bridge";
import { AccountPicker } from "./components/AccountPicker";
import { Dialog } from "./components/Dialog";
import { NewTabForm } from "./components/NewTabForm";
import { SettingsPage } from "./components/SettingsPage";
import { StatusBar } from "./components/StatusBar";
import { TabBar } from "./components/TabBar";
import { destroyTerminal, TerminalView } from "./components/TerminalView";
import { newTabDefaults, type NewTabDraft } from "./newTab";
import { detectPlatform } from "./terminal/keys";
import { dropLivePty, ptyExitCode, ptyRunning, subscribeLivePty, subscribePtyExit } from "./terminal/live";
import { requestTerminalSearch, setCopyOnSelect, setOptionAsMeta, setTerminalTheme } from "./terminal/park";
import { applyTheme, watchSystemTheme } from "./theme";
import { reportError, setErrorNotifier } from "./uiErrors";
import { statusLimit } from "./usage/limits";
import { keyAction, pageAction, type PageAction } from "./pageActions";
import "./App.css";

/** How often the usage line is read again while nothing else asks. */
const USAGE_POLL_MS = 60_000;

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
  applyTheme(view.ui.theme);
  setTerminalTheme(view.ui.theme);
}

function App() {
  const windowId = useRef(currentWindowId()).current;
  const [ctx, setCtx] = useState<WindowContext | null>(null);
  const [settings, setSettings] = useState<SettingsView | null>(null);
  const [state, setState] = useState<AppStateSnapshot | null>(null);
  const [usage, setUsage] = useState<RateWindow[] | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  const [dialog, setDialog] = useState<"new-tab" | "new-window" | "settings" | null>(null);
  const [confirmClose, setConfirmClose] = useState<string | null>(null);
  const [lastFolder, setLastFolder] = useState("");
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
      (snapshot) => setUsage(snapshot.windows),
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

  useEffect(() => subscribeLivePty(() => setLiveVersion((n) => n + 1)), []);
  useEffect(() => subscribePtyExit(() => refreshUsage()), [refreshUsage]);

  const active = state?.tabs.find((tab) => tab.id === state.activeTabId) ?? null;

  const select = (tabId: string) => {
    setState((current) => (current ? { ...current, activeTabId: tabId } : current));
    selectTab(windowId, tabId).catch((err: unknown) => fail("Select tab", err));
  };

  const reload = () => getAppState(windowId).then(setState);

  const create = async (draft: NewTabDraft) => {
    await newTab(windowId, draft.cwd, draft.kind);
    setLastFolder(draft.cwd);
    setDialog(null);
    await reload();
  };

  const doClose = (tabId: string) => {
    setConfirmClose(null);
    closeTab(windowId, tabId).then(
      (next) => {
        destroyTerminal(tabId);
        dropLivePty(tabId);
        setState(next);
      },
      (err: unknown) => fail("Close tab", err),
    );
  };

  // A running terminal asks first; an exited one closes at once.
  const requestClose = (tabId: string) => {
    if (ptyRunning(tabId)) setConfirmClose(tabId);
    else doClose(tabId);
  };

  const reopen = () => {
    reopenClosedTab(windowId).then(
      () => reload(),
      (err: unknown) => fail("Reopen tab", err),
    );
  };

  const rename = (tabId: string, label: string) => {
    renameTab(tabId, label).then(
      () => reload(),
      (err: unknown) => fail("Rename tab", err),
    );
  };

  const run = (action: PageAction) => {
    switch (action.kind) {
      case "new-tab":
        setDialog("new-tab");
        break;
      case "close-tab":
        if (active) requestClose(active.id);
        break;
      case "reopen-closed-tab":
        reopen();
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
      case "select": {
        const tab = state?.tabs[action.index];
        if (tab) select(tab.id);
        break;
      }
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
      keep(
        win.listen<string>("menu", (event) => {
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
      if (dialog) return;
      const action = keyAction(event, platform);
      if (!action) return;
      event.preventDefault();
      runRef.current(action);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [dialog]);

  const exited = new Set((state?.tabs ?? []).filter((tab) => ptyExitCode(tab.id) != null).map((tab) => tab.id));
  const fontSize = settings?.terminal.fontSize ?? 14;

  return (
    <main className="workspace">
      {state && state.tabs.length > 0 && (
        <TabBar
          tabs={state.tabs}
          activeTabId={state.activeTabId}
          exited={exited}
          onSelect={select}
          onClose={requestClose}
          onRename={rename}
          onNew={() => setDialog("new-tab")}
        />
      )}
      <section className="workspace-body">
        {state && state.tabs.length === 0 && (
          <div className="start-screen">
            <h1>New tab</h1>
            <p className="hint">
              {ctx ? `Claude tabs in this window use ${ctx.accountName} (${ctx.config.display}).` : ""}
            </p>
            <NewTabForm initial={newTabDefaults(state, lastFolder)} onCreate={create} />
            {state.closedTabs.length > 0 && (
              <button type="button" className="secondary-button" onClick={reopen}>
                Reopen {state.closedTabs[0].label}
              </button>
            )}
          </div>
        )}
        {active && <TerminalView key={active.id} tab={active} fontSize={fontSize} autoFocus={!dialog} />}
      </section>
      <StatusBar
        account={ctx ? { name: ctx.accountName, config: ctx.config } : null}
        folder={active?.cwd ?? null}
        usage={usage ? statusLimit(usage) : null}
        message={message ? { text: message, onDismiss: () => setMessage(null) } : null}
        onNewWindow={() => setDialog("new-window")}
        onSettings={() => setDialog("settings")}
      />
      {dialog === "new-tab" && (
        <Dialog title="New tab" onClose={() => setDialog(null)}>
          <NewTabForm
            initial={newTabDefaults(state, lastFolder)}
            onCreate={create}
            onCancel={() => setDialog(null)}
          />
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
      {dialog === "settings" && settings && (
        <SettingsPage
          view={settings}
          onSaved={(view) => {
            setSettings(view);
            applySettings(view);
            // An account rename or a new folder retitles this window.
            windowContext().then(setCtx, (err: unknown) => fail("Window", err));
          }}
          onClose={() => setDialog(null)}
        />
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
