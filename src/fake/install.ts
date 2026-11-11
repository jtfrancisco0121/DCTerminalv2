// A faked Tauri backend so the UI renders in a plain browser (`npm run dev:fake`): QA tries the DCTerminalv2 UI
// itself this way. Development only: main.tsx loads this file by a dynamic import behind `MODE === "fake"`, and no
// component imports it (a source-scan test and the `npm run check` dist/ scan enforce both).
//
// It sets `window.__TAURI_INTERNALS__` itself (the @tauri-apps/api mocks are not relied on): `invoke` answers from
// fixtures/fake/*.json, `transformCallback` keeps callbacks, and the event plugin commands are faked so `listen` works.
// A command without a fixture rejects with `fake: <command> not faked`. `fixtures/fake/not-faked.json` lists the
// commands that are faked on purpose with no answer, each with its reason (src/fake/coverage.test.ts keeps both lists
// complete). Four commands have a small behaviour on top of their fixture: `pty_open` pushes a banner through its
// channel, `resume_run` opens the run's tab in the faked app state, and `webqa_targets_save` keeps the saved Web QA targets
// for `get_settings`, and `set_ui_settings` echoes the look it is given.
import app from "../../fixtures/fake/app.json";
import clickup from "../../fixtures/fake/clickup.json";
import run from "../../fixtures/fake/run.json";
import runs from "../../fixtures/fake/runs.json";
import settings from "../../fixtures/fake/settings.json";

export const FAKE_MARKER = "DCV2_FAKE_BACKEND_MARKER";

/** What a faked terminal prints when it opens (base64, as the real PTY packets carry it). */
export const FAKE_BANNER = "DCTerminalv2 faked terminal\r\n(no shell runs in the faked backend)\r\n";

type Answers = Record<string, unknown>;
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

  // The tab list the faked app state shows: `resume_run` opens the run's tab.
  let openedTab: { id: string } | null = null;
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
        const started = fakeAnswer(command, fixtures) as { tab: { id: string } };
        openedTab = started.tab;
        return started;
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
      case "get_app_state": {
        const state = fakeAnswer(command, fixtures) as { activeTabId: string | null; tabs: unknown[] };
        return openedTab ? { ...state, activeTabId: openedTab.id, tabs: [openedTab] } : state;
      }
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
