import { describe, expect, it } from "vitest";
import { detectPlatform, isAppShortcut, isShiftEnter, SHIFT_ENTER_SEQUENCE, shiftEnterSendsNewline } from "./keys";

const ev = (over: Partial<Parameters<typeof isShiftEnter>[0]> = {}) => ({
  type: "keydown",
  key: "Enter",
  ctrlKey: false,
  metaKey: false,
  altKey: false,
  shiftKey: false,
  ...over,
});

describe("terminal Shift+Enter", () => {
  it("sends ESC then CR, the sequence Claude Code accepts as a new line", () => {
    expect(SHIFT_ENTER_SEQUENCE).toBe("\u001b\r");
  });

  it("matches Shift+Enter alone", () => {
    expect(isShiftEnter(ev({ shiftKey: true }))).toBe(true);
  });

  it("leaves plain Enter and other modified Enters to xterm", () => {
    expect(isShiftEnter(ev())).toBe(false);
    expect(isShiftEnter(ev({ shiftKey: true, ctrlKey: true }))).toBe(false);
    expect(isShiftEnter(ev({ shiftKey: true, altKey: true }))).toBe(false);
    expect(isShiftEnter(ev({ shiftKey: true, metaKey: true }))).toBe(false);
    expect(isShiftEnter(ev({ shiftKey: true, key: "a" }))).toBe(false);
  });

  it("ignores Enter that confirms an IME composition", () => {
    expect(isShiftEnter(ev({ shiftKey: true, isComposing: true }))).toBe(false);
  });

  it("rewrites Shift+Enter only for Claude terminals", () => {
    expect(shiftEnterSendsNewline("claude")).toBe(true);
    expect(shiftEnterSendsNewline("shell")).toBe(false);
  });
});

describe("app shortcuts in a terminal", () => {
  it("keeps Cmd+T/W/N/,/F and Cmd+digit for the app on macOS", () => {
    for (const key of ["t", "w", "n", ",", "f", "3"]) {
      expect(isAppShortcut(ev({ key, metaKey: true }), "mac"), key).toBe(true);
    }
    expect(isAppShortcut(ev({ key: "T", metaKey: true, shiftKey: true }), "mac")).toBe(true);
  });

  it("leaves Ctrl keys to the program on macOS and plain Ctrl keys elsewhere", () => {
    expect(isAppShortcut(ev({ key: "w", ctrlKey: true }), "mac")).toBe(false);
    expect(isAppShortcut(ev({ key: "c", metaKey: true }), "mac")).toBe(false);
    expect(isAppShortcut(ev({ key: "w", ctrlKey: true }), "linux")).toBe(false);
    expect(isAppShortcut(ev({ key: "w", ctrlKey: true, shiftKey: true }), "windows")).toBe(true);
    expect(isAppShortcut(ev({ type: "keyup", key: "t", metaKey: true }), "mac")).toBe(false);
  });

  it("reads the platform from navigator.platform", () => {
    expect(detectPlatform("MacIntel")).toBe("mac");
    expect(detectPlatform("Win32")).toBe("windows");
    expect(detectPlatform("Linux x86_64")).toBe("linux");
    expect(detectPlatform(undefined)).toBe("linux");
  });
});
