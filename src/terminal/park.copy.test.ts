// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

let selectionChanged: (() => void) | null = null;

// The real reporter behind a spy, so its once-keys are exercised too.
vi.mock("../uiErrors", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../uiErrors")>();
  return { ...actual, reportError: vi.fn(actual.reportError) };
});
vi.mock("../bridge", () => ({ ptySetScheme: vi.fn(() => Promise.resolve()) }));
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }));
vi.mock("@xterm/addon-fit", () => ({ FitAddon: class {} }));
vi.mock("@xterm/addon-search", () => ({ SearchAddon: class {} }));
vi.mock("@xterm/addon-web-links", () => ({ WebLinksAddon: class {} }));
vi.mock("@xterm/xterm", () => ({
  Terminal: class {
    options = {};
    loadAddon() {}
    registerLinkProvider() {}
    registerLinkProvider() {}
    onSelectionChange(handler: () => void) {
      selectionChanged = handler;
    }
    hasSelection() {
      return true;
    }
    getSelection() {
      return "selected text";
    }
    onData() {
      return { dispose() {} };
    }
    open() {}
  },
}));

import { clearUiErrors, reportError, setErrorNotifier } from "../uiErrors";
import { ensureParkedTerminal, setCopyOnSelect } from "./park";

describe("copy on select", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.mocked(reportError).mockClear();
    clearUiErrors();
  });
  afterEach(() => {
    setErrorNotifier(null);
    vi.useRealTimers();
    vi.unstubAllGlobals();
    setCopyOnSelect(false);
  });

  it("a copy the clipboard rejects is shown, once per terminal", async () => {
    const error = new Error("clipboard denied");
    vi.stubGlobal("navigator", { ...navigator, clipboard: { writeText: vi.fn(() => Promise.reject(error)) } });
    setCopyOnSelect(true);
    ensureParkedTerminal("t1", 13);
    selectionChanged!();
    await vi.advanceTimersByTimeAsync(250);
    expect(navigator.clipboard.writeText).toHaveBeenCalledWith("selected text");
    expect(reportError).toHaveBeenCalledWith("Copy on select failed", error, {
      notify: true,
      once: "Copy on select failed:t1",
    });
  });

  it("a rejected copy in a second terminal is shown too, a repeat in the first is not", async () => {
    const notify = vi.fn();
    setErrorNotifier(notify);
    vi.stubGlobal("navigator", {
      ...navigator,
      clipboard: { writeText: vi.fn(() => Promise.reject(new Error("clipboard denied"))) },
    });
    setCopyOnSelect(true);
    ensureParkedTerminal("copy-a", 13);
    const selectInFirst = selectionChanged!;
    ensureParkedTerminal("copy-b", 13);
    const selectInSecond = selectionChanged!;
    selectInFirst();
    await vi.advanceTimersByTimeAsync(250);
    selectInFirst();
    await vi.advanceTimersByTimeAsync(250);
    selectInSecond();
    await vi.advanceTimersByTimeAsync(250);
    expect(notify).toHaveBeenCalledTimes(2);
    expect(notify).toHaveBeenCalledWith("Copy on select failed", "clipboard denied");
  });
});
