// A faked Tauri backend so the UI renders in a plain browser (`npm run dev:fake`): QA tries the DCTerminalv2 UI
// itself this way. Development only: main.tsx loads this file by a dynamic import behind `MODE === "fake"`, and no
// component imports it (a source-scan test and the `npm run check` dist/ scan enforce both).
//
// It sets `window.__TAURI_INTERNALS__` itself (the @tauri-apps/api mocks are not relied on): `invoke` answers from
// fixtures/fake/*.json, `transformCallback` keeps callbacks, and the event plugin commands are faked so `listen` works.
// A command without a fixture rejects with `fake: <command> not faked`. `fixtures/fake/not-faked.json` lists the
// commands that are faked on purpose with no answer, each with its reason (src/fake/coverage.test.ts keeps both lists
// complete). Some commands have a small behaviour on top of their fixture: `pty_open` pushes a banner through its
// channel (the banner prints the fake run's plan.md, so Cmd+click on a path can be tried); `resume_run` and `webqa_start`
// open the run's tab in the faked app state, which `get_app_state` lists (the newest opened tab active); `run_tabs`
// answers only the readings of opened tabs; `paths_exist` answers only the candidates asked for; `webqa_targets_save`
// keeps the saved Web QA targets for `get_settings`; and `set_ui_settings` echoes the look it is given.
// The commands in `BEHAVIOUR_ONLY` have no fixture of their own (no Rust sample in src-tauri/src/fake_shapes.rs): their
// answer is made here. `new_tab` adds a plain tab shaped like the `resume_run` fixture's tab, `select_tab`, `close_tab`,
// `move_tab` and `rename_tab` change the faked tab list, and `preview_file` / `preview_stat` read `FAKE_FILES`.
import app from "../../fixtures/fake/app.json";
import clickup from "../../fixtures/fake/clickup.json";
import run from "../../fixtures/fake/run.json";
import runs from "../../fixtures/fake/runs.json";
import settings from "../../fixtures/fake/settings.json";

export const FAKE_MARKER = "DCV2_FAKE_BACKEND_MARKER";

/** The files the faked backend has: `preview_file` / `preview_stat` read them, and the `paths_exist` fixture links them. */
export const FAKE_FILES: Record<string, string> = {
  "/fake/runs/r-20261022-1304/plan.md":
    "# Plan: r-20261022-1304\n\nA faked run plan for the preview pane.\n\n## PR1 mathlib\n\n- S1.1 Start button visible\n- S1.2 Run panel shows Planning\n",
};

/** When every faked file was last changed (ms). */
const FAKE_FILE_MTIME = Date.UTC(2026, 9, 22, 13, 40);

/** What a faked terminal prints when it opens (base64, as the real PTY packets carry it). */
export const FAKE_BANNER =
  "DCTerminalv2 faked terminal\r\n(no shell runs in the faked backend)\r\n" +
  "Plan: /fake/runs/r-20261022-1304/plan.md\r\n";

/** Commands faked by behaviour alone: no fixture, the answer is made in `installFakeBackend`. */
export const BEHAVIOUR_ONLY = [
  "new_tab",
  "select_tab",
  "close_tab",
  "move_tab",
  "rename_tab",
  "preview_file",
  "preview_stat",
] as const;

type Answers = Record<string, unknown>;
/** A tab of the faked app state (a TabSummary of src/bridge.ts, loosely). */
type FakeTab = { id: string } & Record<string, unknown>;
type Handler = (payload: unknown) => void;

const FIXTURES: Answers = { ...app, ...settings, ...run, ...runs, ...clickup };

/** The fixture for `command`, a fresh copy so a component can never change the fixture. */
export function fakeAnswer(
  command: string,
  fixtures: Answers = FIXTURES,
): unknown {
  if (!Object.prototype.hasOwnProperty.call(fixtures, command)) {
    throw new Error(`fake: ${command} not faked`);
  }
  return JSON.parse(JSON.stringify(fixtures[command]));
}

type FakeWindow = Window & {
  __TAURI_INTERNALS__?: unknown;
  __TAURI_EVENT_PLUGIN_INTERNALS__?: unknown;
  __DCV2_FAKE__?: string;
};

/** Installs the fake into `target` (the page's window; a test passes its own object). */
export function installFakeBackend(
  target: FakeWindow = window as FakeWindow,
  fixtures: Answers = FIXTURES,
): void {
  const callbacks = new Map<number, Handler>();
  const listeners = new Map<string, Array<{ id: number; handler: number }>>();
  let nextCallback = 1;
  let nextEvent = 1;

  const emit = (event: string, payload: unknown) => {
    for (const l of listeners.get(event) ?? []) {
      callbacks.get(l.handler)?.({ event, id: l.id, payload });
    }
  };

  // The tab list the faked app state shows, in tab order: `resume_run` opens the run's tab, `new_tab` a plain tab.
  // Empty: `get_app_state` answers its fixture as it is.
  let openedTabs: FakeTab[] = [];
  let activeTabId: string | null = null;
  let nextTab = 1;
  const openTab = (tab: FakeTab) => {
    openedTabs = [...openedTabs.filter((t) => t.id !== tab.id), tab];
    activeTabId = tab.id;
  };
  const appState = () => {
    const state = fakeAnswer("get_app_state", fixtures) as { activeTabId: string | null; tabs: unknown[] };
    return openedTabs.length > 0
      ? { ...state, activeTabId, tabs: JSON.parse(JSON.stringify(openedTabs)) as unknown[] }
      : state;
  };
  // The Web QA targets saved in this session: `get_settings` shows them, so Settings and Runs agree (the fixture has none).
  let webQaTargets: unknown[] | null = null;
  // The look saved in this session (`set_ui_settings` echoes it, `get_settings` shows it).
  let savedUi: Record<string, unknown> | null = null;

  const invoke = async (
    command: string,
    args?: Record<string, unknown>,
  ): Promise<unknown> => {
    switch (command) {
      case "pty_open": {
        // The terminal's output channel: push one banner packet through its callback.
        const channel = args?.onOutput as { id?: number } | undefined;
        const handler = channel?.id === undefined ? undefined : callbacks.get(channel.id);
        if (handler) {
          const data = btoa(FAKE_BANNER);
          setTimeout(() => handler({ message: { kind: "output", data, code: null }, index: 0, id: 0 }), 0);
        }
        return fakeAnswer(command, fixtures);
      }
      case "webqa_start":
      case "resume_run": {
        const started = fakeAnswer(command, fixtures) as { tab: FakeTab };
        openTab(JSON.parse(JSON.stringify(started.tab)) as FakeTab);
        return started;
      }
      case "new_tab": {
        // A plain tab in `cwd`, made active; shaped like the `resume_run` fixture's tab (a real TabSummary).
        const template = (fakeAnswer("resume_run", fixtures) as { tab: FakeTab }).tab;
        const cwd = String(args?.cwd ?? "");
        const kind = args?.kind === "shell" ? "shell" : "claude";
        const folder = cwd.replace(/[/\\]+$/, "").split(/[/\\]/).pop() || cwd;
        const tab: FakeTab = {
          ...template,
          id: `tab_fake_${nextTab++}`,
          label: `${kind === "shell" ? "Shell" : "Claude"} · ${folder}`,
          customLabel: false,
          cwd,
          kind,
          folderStatus: "ok",
          hasSession: false,
          runDir: null,
          role: null,
          split: null,
          pendingStart: false,
        };
        openTab(tab);
        return JSON.parse(JSON.stringify(tab));
      }
      case "select_tab": {
        const tabId = String(args?.tabId);
        if (!openedTabs.some((t) => t.id === tabId)) throw new Error(`fake: no tab ${tabId}`);
        activeTabId = tabId;
        return null;
      }
      case "close_tab": {
        // The tab goes; the newest remaining tab becomes active. Nothing joins `closedTabs` (reopening is not faked).
        const tabId = String(args?.tabId);
        openedTabs = openedTabs.filter((t) => t.id !== tabId);
        if (activeTabId === tabId) activeTabId = openedTabs[openedTabs.length - 1]?.id ?? null;
        const state = fakeAnswer("get_app_state", fixtures) as Record<string, unknown>;
        return { ...state, activeTabId, tabs: JSON.parse(JSON.stringify(openedTabs)) as unknown[] };
      }
      case "move_tab": {
        // Drag `tabId` to `index` among the tabs; answers the new state.
        const tabId = String(args?.tabId);
        const moved = openedTabs.find((t) => t.id === tabId);
        if (!moved) throw new Error(`fake: no tab ${tabId}`);
        const rest = openedTabs.filter((t) => t.id !== tabId);
        const index = Math.max(0, Math.min(rest.length, Number(args?.index) || 0));
        openedTabs = [...rest.slice(0, index), moved, ...rest.slice(index)];
        return appState();
      }
      case "rename_tab": {
        const tabId = String(args?.tabId);
        const tab = openedTabs.find((t) => t.id === tabId);
        if (!tab) throw new Error(`fake: no tab ${tabId}`);
        const label = String(args?.label ?? "").trim();
        if (label) Object.assign(tab, { label, customLabel: true });
        return JSON.parse(JSON.stringify(tab));
      }
      case "run_tabs": {
        // Only the readings of tabs this session opened, as the real list holds only open run tabs.
        const readings = fakeAnswer(command, fixtures) as Array<{ tabId: string }>;
        return readings.filter((r) => openedTabs.some((t) => t.id === r.tabId));
      }
      case "paths_exist": {
        // The fixture's hits for the candidates asked for: a faked file is linked, anything else is not.
        const asked = new Set((args?.candidates as string[] | undefined) ?? []);
        return (fakeAnswer(command, fixtures) as Array<{ candidate: string }>).filter((hit) => asked.has(hit.candidate));
      }
      case "preview_file": {
        const path = String(args?.path ?? "");
        const text = FAKE_FILES[path];
        if (text === undefined) throw new Error("file not found");
        return { path, text, truncated: false, mtimeMs: FAKE_FILE_MTIME };
      }
      case "preview_stat": {
        const text = FAKE_FILES[String(args?.path ?? "")];
        return text === undefined
          ? { exists: false, mtimeMs: 0, size: 0 }
          : { exists: true, mtimeMs: FAKE_FILE_MTIME, size: new TextEncoder().encode(text).length };
      }
      case "webqa_targets_save": {
        const saved = fakeAnswer(command, fixtures) as { targets: unknown[] };
        webQaTargets = JSON.parse(JSON.stringify(args?.targets ?? []));
        return { ...saved, targets: JSON.parse(JSON.stringify(webQaTargets)) };
      }
      case "set_ui_settings": {
        // Echo the saved look, and keep it for `get_settings` so the page shows what a click chose.
        const shape = fakeAnswer(command, fixtures) as Record<string, unknown>;
        savedUi = { ...shape, ...JSON.parse(JSON.stringify(args?.ui ?? {})) };
        return JSON.parse(JSON.stringify(savedUi));
      }
      case "get_settings": {
        const view = fakeAnswer(command, fixtures) as { webQa?: Record<string, unknown>; ui?: Record<string, unknown> };
        const shown = savedUi ? { ...view, ui: JSON.parse(JSON.stringify(savedUi)) } : view;
        return webQaTargets
          ? { ...shown, webQa: { ...view.webQa, targets: JSON.parse(JSON.stringify(webQaTargets)) } }
          : shown;
      }
      case "get_app_state":
        return appState();
      case "plugin:event|listen": {
        const id = nextEvent++;
        const event = String(args?.event);
        listeners.set(event, [
          ...(listeners.get(event) ?? []),
          { id, handler: Number(args?.handler) },
        ]);
        return id;
      }
      case "plugin:event|unlisten": {
        const event = String(args?.event);
        listeners.set(
          event,
          (listeners.get(event) ?? []).filter(
            (l) => l.id !== Number(args?.eventId),
          ),
        );
        return null;
      }
      case "plugin:event|emit":
      case "plugin:event|emit_to":
        emit(String(args?.event), args?.payload);
        return null;
      default:
        return fakeAnswer(command, fixtures);
    }
  };

  target.__TAURI_INTERNALS__ = {
    invoke,
    transformCallback: (callback?: Handler, once?: boolean) => {
      const id = nextCallback++;
      callbacks.set(id, (payload) => {
        if (once) callbacks.delete(id);
        callback?.(payload);
      });
      return id;
    },
    unregisterCallback: (id: number) => {
      callbacks.delete(id);
    },
    convertFileSrc: (path: string) => path,
    metadata: {
      currentWindow: { label: "main" },
      currentWebview: { windowLabel: "main", label: "main" },
    },
  };
  target.__TAURI_EVENT_PLUGIN_INTERNALS__ = {
    unregisterListener: (event: string, id: number) => {
      listeners.set(
        event,
        (listeners.get(event) ?? []).filter((l) => l.id !== id),
      );
    },
  };
  target.__DCV2_FAKE__ = FAKE_MARKER;
}
