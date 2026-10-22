import { describe, expect, it } from "vitest";
import { keyAction, pageAction } from "./pageActions";

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
