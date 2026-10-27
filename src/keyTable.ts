/**
 * The app's keyboard shortcuts, in one place. `pageActions.keyAction` (what
 * the page does) and `terminal/keys.isAppShortcut` (what xterm must leave
 * alone) both read `lookupShortcut`, so they cannot disagree.
 *
 * Cmd on macOS; Ctrl+Shift elsewhere (plain Ctrl keys belong to the shell),
 * except Ctrl+Tab and Ctrl+Shift+Tab, which cycle the tabs on every platform.
 * On macOS the native menu has its own accelerator for most of them and sends
 * the page a `menu` event, so the page ignores those keys itself (`menu`).
 */

import type { PageAction } from "./pageActions";
import type { Platform } from "./terminal/keys";

export type KeyEvent = Pick<KeyboardEvent, "key" | "metaKey" | "ctrlKey" | "altKey" | "shiftKey">;

export type Binding = {
  action: PageAction;
  /** macOS: a menu accelerator takes this key, so the page does not act on it. */
  menu: boolean;
};

/** The font size zoom keys move within, and Cmd+0 returns to. */
export const MIN_FONT_SIZE = 8;
export const MAX_FONT_SIZE = 32;
export const DEFAULT_FONT_SIZE = 14;

const ARROWS: Record<string, "left" | "right" | "up" | "down" | undefined> = {
  ArrowLeft: "left",
  ArrowRight: "right",
  ArrowUp: "up",
  ArrowDown: "down",
};

export function lookupShortcut(event: KeyEvent, platform: Platform): Binding | null {
  const mac = platform === "mac";
  // Ctrl+Tab cycles the tabs everywhere (the page handles it, no menu).
  if (event.key === "Tab" && event.ctrlKey && !event.metaKey && !event.altKey) {
    return { action: { kind: event.shiftKey ? "prev-tab" : "next-tab" }, menu: false };
  }
  // Moving focus between the two panes of a tab: Cmd+Option+Arrow (macOS),
  // Ctrl+Shift+Alt+Arrow elsewhere. Split down elsewhere is Ctrl+Shift+Alt+D.
  const withAlt = mac
    ? event.metaKey && !event.ctrlKey && event.altKey
    : event.ctrlKey && event.shiftKey && !event.metaKey && event.altKey;
  if (withAlt) {
    const direction = ARROWS[event.key];
    if (direction) return { action: { kind: "focus-pane", direction }, menu: false };
    if (!mac && event.key.toLowerCase() === "d") return { action: { kind: "split", direction: "down" }, menu: false };
    return null;
  }
  const command = mac
    ? event.metaKey && !event.ctrlKey && !event.altKey
    : event.ctrlKey && event.shiftKey && !event.metaKey && !event.altKey;
  if (!command) return null;
  const key = event.key.toLowerCase();
  const plain = !event.shiftKey;
  if (key === "f") return { action: { kind: "find" }, menu: false };
  if (/^[1-9]$/.test(key)) return { action: { kind: "select", index: Number(key) - 1 }, menu: false };
  if (event.shiftKey && (key === "]" || key === "}")) return { action: { kind: "next-tab" }, menu: mac };
  if (event.shiftKey && (key === "[" || key === "{")) return { action: { kind: "prev-tab" }, menu: mac };
  // "=" is the zoom-in key without Shift; "+" is what Shift+= types.
  if (key === "=" || key === "+") return { action: { kind: "zoom-in" }, menu: mac && key === "=" && plain };
  if (key === "-" || key === "_") return { action: { kind: "zoom-out" }, menu: mac && key === "-" && plain };
  if (key === "0" || key === ")") return { action: { kind: "zoom-reset" }, menu: mac && key === "0" && plain };
  // Split: Cmd+D right, Cmd+Shift+D down (macOS); Ctrl+Shift+D right elsewhere.
  if (key === "d") {
    return { action: { kind: "split", direction: mac && event.shiftKey ? "down" : "right" }, menu: false };
  }
  if (key === "t") {
    return { action: { kind: mac && event.shiftKey ? "reopen-closed-tab" : "new-tab" }, menu: mac };
  }
  if (key === "w") return { action: { kind: "close-tab" }, menu: mac };
  if (key === "n") return { action: { kind: "new-window" }, menu: mac };
  if (key === ",") return { action: { kind: "settings" }, menu: mac };
  return null;
}

/** The next font size for a zoom action, kept within 8..32; null when it is not one. */
export function zoomedFontSize(current: number, action: PageAction["kind"]): number | null {
  const clamp = (n: number) => Math.min(MAX_FONT_SIZE, Math.max(MIN_FONT_SIZE, n));
  switch (action) {
    case "zoom-in":
      return clamp(current + 1);
    case "zoom-out":
      return clamp(current - 1);
    case "zoom-reset":
      return DEFAULT_FONT_SIZE;
    default:
      return null;
  }
}

/**
 * One sample key press per shortcut and platform, for the agreement test
 * (every sample must be an app shortcut for xterm and give the same action
 * for the page).
 */
export const SHORTCUT_SAMPLES: ReadonlyArray<{
  name: string;
  action: PageAction;
  mac: KeyEvent;
  /** Null: there is no such shortcut outside macOS. */
  other: KeyEvent | null;
  /** macOS leaves this key to the native menu. */
  macMenu: boolean;
}> = (() => {
  const ev = (key: string, over: Partial<KeyEvent> = {}): KeyEvent => ({
    key,
    metaKey: false,
    ctrlKey: false,
    altKey: false,
    shiftKey: false,
    ...over,
  });
  const cmd = (key: string, over: Partial<KeyEvent> = {}) => ev(key, { metaKey: true, ...over });
  const cs = (key: string, over: Partial<KeyEvent> = {}) => ev(key, { ctrlKey: true, shiftKey: true, ...over });
  return [
    { name: "find", action: { kind: "find" }, mac: cmd("f"), other: cs("F"), macMenu: false },
    { name: "select 3", action: { kind: "select", index: 2 }, mac: cmd("3"), other: cs("3"), macMenu: false },
    { name: "new tab", action: { kind: "new-tab" }, mac: cmd("t"), other: cs("T"), macMenu: true },
    { name: "close tab", action: { kind: "close-tab" }, mac: cmd("w"), other: cs("W"), macMenu: true },
    { name: "new window", action: { kind: "new-window" }, mac: cmd("n", { shiftKey: true }), other: cs("N"), macMenu: true },
    { name: "settings", action: { kind: "settings" }, mac: cmd(","), other: cs(","), macMenu: true },
    { name: "reopen closed tab", action: { kind: "reopen-closed-tab" }, mac: cmd("T", { shiftKey: true }), other: null, macMenu: true },
    { name: "next tab (Ctrl+Tab)", action: { kind: "next-tab" }, mac: ev("Tab", { ctrlKey: true }), other: ev("Tab", { ctrlKey: true }), macMenu: false },
    { name: "previous tab (Ctrl+Shift+Tab)", action: { kind: "prev-tab" }, mac: ev("Tab", { ctrlKey: true, shiftKey: true }), other: ev("Tab", { ctrlKey: true, shiftKey: true }), macMenu: false },
    { name: "next tab (bracket)", action: { kind: "next-tab" }, mac: cmd("}", { shiftKey: true }), other: cs("}"), macMenu: true },
    { name: "previous tab (bracket)", action: { kind: "prev-tab" }, mac: cmd("{", { shiftKey: true }), other: cs("{"), macMenu: true },
    { name: "zoom in (=)", action: { kind: "zoom-in" }, mac: cmd("="), other: cs("+"), macMenu: true },
    { name: "zoom in (+)", action: { kind: "zoom-in" }, mac: cmd("+", { shiftKey: true }), other: cs("+"), macMenu: false },
    { name: "zoom out", action: { kind: "zoom-out" }, mac: cmd("-"), other: cs("_"), macMenu: true },
    { name: "zoom reset", action: { kind: "zoom-reset" }, mac: cmd("0"), other: cs(")"), macMenu: true },
    { name: "split right", action: { kind: "split", direction: "right" }, mac: cmd("d"), other: cs("D"), macMenu: false },
    {
      name: "split down",
      action: { kind: "split", direction: "down" },
      mac: cmd("D", { shiftKey: true }),
      other: ev("D", { ctrlKey: true, shiftKey: true, altKey: true }),
      macMenu: false,
    },
    ...(["Left", "Right", "Up", "Down"] as const).map((name) => ({
      name: `focus pane ${name.toLowerCase()}`,
      action: { kind: "focus-pane", direction: name.toLowerCase() } as PageAction,
      mac: cmd(`Arrow${name}`, { altKey: true }),
      other: ev(`Arrow${name}`, { ctrlKey: true, shiftKey: true, altKey: true }),
      macMenu: false,
    })),
  ];
})();
