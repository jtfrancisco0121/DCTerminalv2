import { describe, expect, it } from "vitest";
import { SHORTCUT_SAMPLES, zoomedFontSize } from "./keyTable";
import { keyAction, pageAction } from "./pageActions";
import { isAppShortcut } from "./terminal/keys";

const key = (k: string, over: Partial<KeyboardEvent> = {}) => ({
  key: k,
  metaKey: false,
  ctrlKey: false,
  altKey: false,
  shiftKey: false,
  ...over,
});

describe("menu items", () => {
  it("map the native menu's ids to page actions", () => {
    expect(pageAction("new-tab")).toEqual({ kind: "new-tab" });
    expect(pageAction("close-tab")).toEqual({ kind: "close-tab" });
    expect(pageAction("reopen-closed-tab")).toEqual({ kind: "reopen-closed-tab" });
    expect(pageAction("settings")).toEqual({ kind: "settings" });
    expect(pageAction("next-tab")).toEqual({ kind: "next-tab" });
    expect(pageAction("prev-tab")).toEqual({ kind: "prev-tab" });
    expect(pageAction("zoom-in")).toEqual({ kind: "zoom-in" });
    expect(pageAction("zoom-out")).toEqual({ kind: "zoom-out" });
    expect(pageAction("zoom-reset")).toEqual({ kind: "zoom-reset" });
    expect(pageAction("reopen-closed:tab_17")).toEqual({ kind: "reopen-closed", tabId: "tab_17" });
    expect(pageAction("reopen-closed:")).toBeNull();
    expect(pageAction("quit")).toBeNull();
    expect(pageAction(7)).toBeNull();
  });
});

describe("page shortcuts", () => {
  it("on macOS: Cmd+F and Cmd+digit only; the menu has the rest", () => {
    expect(keyAction(key("f", { metaKey: true }), "mac")).toEqual({ kind: "find" });
    expect(keyAction(key("3", { metaKey: true }), "mac")).toEqual({ kind: "select", index: 2 });
    expect(keyAction(key("t", { metaKey: true }), "mac")).toBeNull();
    expect(keyAction(key("f", { ctrlKey: true }), "mac")).toBeNull();
  });

  it("elsewhere: Ctrl+Shift for every page shortcut, plain Ctrl stays with the terminal", () => {
    const ctrlShift = { ctrlKey: true, shiftKey: true };
    expect(keyAction(key("T", ctrlShift), "linux")).toEqual({ kind: "new-tab" });
    expect(keyAction(key("w", ctrlShift), "windows")).toEqual({ kind: "close-tab" });
    expect(keyAction(key("n", ctrlShift), "windows")).toEqual({ kind: "new-window" });
    expect(keyAction(key(",", ctrlShift), "linux")).toEqual({ kind: "settings" });
    expect(keyAction(key("w", { ctrlKey: true }), "linux")).toBeNull();
  });
});

describe("tab cycling and font zoom keys", () => {
  it("Ctrl+Tab and Ctrl+Shift+Tab cycle on every platform; the menu keys are left to the menu on macOS", () => {
    for (const platform of ["mac", "linux", "windows"] as const) {
      expect(keyAction(key("Tab", { ctrlKey: true }), platform)).toEqual({ kind: "next-tab" });
      expect(keyAction(key("Tab", { ctrlKey: true, shiftKey: true }), platform)).toEqual({ kind: "prev-tab" });
    }
    expect(keyAction(key("Tab"), "linux")).toBeNull();
    expect(keyAction(key("Tab", { metaKey: true }), "mac")).toBeNull();
    // macOS: the menu has Cmd+Shift+] / [ and Cmd+= / - / 0; Shift+= (a "+") is the page's.
    expect(keyAction(key("}", { metaKey: true, shiftKey: true }), "mac")).toBeNull();
    expect(keyAction(key("=", { metaKey: true }), "mac")).toBeNull();
    expect(keyAction(key("0", { metaKey: true }), "mac")).toBeNull();
    expect(keyAction(key("+", { metaKey: true, shiftKey: true }), "mac")).toEqual({ kind: "zoom-in" });
    // Elsewhere: the Ctrl+Shift forms.
    const cs = { ctrlKey: true, shiftKey: true };
    expect(keyAction(key("}", cs), "linux")).toEqual({ kind: "next-tab" });
    expect(keyAction(key("[", cs), "windows")).toEqual({ kind: "prev-tab" });
    expect(keyAction(key("+", cs), "linux")).toEqual({ kind: "zoom-in" });
    expect(keyAction(key("_", cs), "linux")).toEqual({ kind: "zoom-out" });
    expect(keyAction(key(")", cs), "linux")).toEqual({ kind: "zoom-reset" });
    expect(keyAction(key("=", { ctrlKey: true }), "linux")).toBeNull();
  });

  it("zooms the font by one within 8..32 and resets to 14", () => {
    expect(zoomedFontSize(14, "zoom-in")).toBe(15);
    expect(zoomedFontSize(32, "zoom-in")).toBe(32);
    expect(zoomedFontSize(8, "zoom-out")).toBe(8);
    expect(zoomedFontSize(20, "zoom-out")).toBe(19);
    expect(zoomedFontSize(9, "zoom-reset")).toBe(14);
    expect(zoomedFontSize(14, "find")).toBeNull();
  });

  it("the page and the terminal agree on every listed key, on macOS and elsewhere", () => {
    const asTerminal = (e: ReturnType<typeof key>) => ({ type: "keydown", ...e });
    for (const sample of SHORTCUT_SAMPLES) {
      // xterm leaves every listed key to the app.
      expect(isAppShortcut(asTerminal(sample.mac), "mac"), `${sample.name} (mac, terminal)`).toBe(true);
      if (!sample.other) continue;
      expect(isAppShortcut(asTerminal(sample.other), "linux"), `${sample.name} (linux, terminal)`).toBe(true);
      expect(isAppShortcut(asTerminal(sample.other), "windows"), `${sample.name} (windows, terminal)`).toBe(true);
      // The page does the action, except the keys the macOS menu takes.
      expect(keyAction(sample.mac, "mac"), `${sample.name} (mac, page)`).toEqual(sample.macMenu ? null : sample.action);
      if (!sample.other) continue;
      expect(keyAction(sample.other, "linux"), `${sample.name} (linux, page)`).toEqual(sample.action);
      expect(keyAction(sample.other, "windows"), `${sample.name} (windows, page)`).toEqual(sample.action);
    }
    // Keys that are not shortcuts stay with the program on both sides.
    for (const plain of [key("c", { ctrlKey: true }), key("Tab"), key("t"), key("=", { ctrlKey: true })]) {
      expect(isAppShortcut({ type: "keydown", ...plain }, "linux")).toBe(false);
      expect(keyAction(plain, "linux")).toBeNull();
    }
    expect(isAppShortcut({ type: "keyup", ...key("Tab", { ctrlKey: true }) }, "mac")).toBe(false);
  });
});
