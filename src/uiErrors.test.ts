import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { clearUiErrors, reportError, setErrorNotifier, uiErrors } from "./uiErrors";

describe("reportError", () => {
  const notify = vi.fn();
  beforeEach(() => {
    clearUiErrors();
    notify.mockClear();
    setErrorNotifier(notify);
  });
  afterEach(() => setErrorNotifier(null));

  it("writes every error to the error list, and a user action also to a toast", () => {
    reportError("Copy failed", new Error("clipboard denied"), { notify: true });
    reportError("Copy failed", "clipboard denied again", { notify: true });
    expect(uiErrors()).toEqual([
      expect.objectContaining({ source: "Copy failed", message: "clipboard denied" }),
      expect.objectContaining({ source: "Copy failed", message: "clipboard denied again" }),
    ]);
    expect(notify).toHaveBeenCalledTimes(2);
    expect(notify).toHaveBeenCalledWith("Copy failed", "clipboard denied");
  });

  it("without notify only the error list hears of it", () => {
    reportError("Second pane", new Error("boom"));
    expect(uiErrors()).toHaveLength(1);
    expect(notify).not.toHaveBeenCalled();
  });

  it("a repeating background failure is reported once per source", () => {
    for (let i = 0; i < 5; i += 1) {
      reportError("Layout not saved", new Error(`disk full ${i}`), { notify: true, once: true });
    }
    reportError("Keystrokes not sent", new Error("no terminal"), { notify: true, once: true });
    expect(uiErrors().map((entry) => entry.message)).toEqual(["disk full 0", "no terminal"]);
    expect(notify).toHaveBeenCalledTimes(2);
    expect(notify).toHaveBeenNthCalledWith(1, "Layout not saved", "disk full 0");
  });

  it("a string `once` keys the report on itself, so two calls can share one report", () => {
    reportError("Model settings did not load", "a", { notify: true, once: "modelsLoad" });
    reportError("Model settings did not load", "b", { notify: true, once: "modelsLoad" });
    expect(notify).toHaveBeenCalledTimes(1);
  });

  it("an object error with a message reads as that message", () => {
    reportError("Stop", { message: "pty gone" }, { notify: true });
    expect(notify).toHaveBeenCalledWith("Stop", "pty gone");
  });
});
