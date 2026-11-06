// A faked Tauri backend so the UI renders in a plain browser (`npm run dev:fake`): QA tries the DCTerminalv2 UI
// itself this way. Development only: main.tsx loads this file by a dynamic import behind `MODE === "fake"`, and no
// component imports it (a source-scan test and the `npm run check` dist/ scan enforce both).
//
// It sets `window.__TAURI_INTERNALS__` itself (the @tauri-apps/api mocks are not relied on): `invoke` answers from
// fixtures/fake/*.json, `transformCallback` keeps callbacks, and the event plugin commands are faked so `listen` works.
// A command without a fixture rejects with `fake: <command> not faked`.
import app from "../../fixtures/fake/app.json";
import clickup from "../../fixtures/fake/clickup.json";
import run from "../../fixtures/fake/run.json";
import runs from "../../fixtures/fake/runs.json";
import settings from "../../fixtures/fake/settings.json";

export const FAKE_MARKER = "DCV2_FAKE_BACKEND_MARKER";

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

  const invoke = async (
    command: string,
    args?: Record<string, unknown>,
  ): Promise<unknown> => {
    switch (command) {
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
