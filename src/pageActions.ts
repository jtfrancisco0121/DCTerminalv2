/**
 * What the page does for a menu item (the native macOS menu sends its id)
 * or a shortcut it handles itself.
 */

import { lookupShortcut, type KeyEvent } from "./keyTable";
import type { Platform } from "./terminal/keys";

export type PageAction =
  | { kind: "new-tab" }
  | { kind: "close-tab" }
  | { kind: "reopen-closed-tab" }
  | { kind: "settings" }
  | { kind: "new-window" }
  | { kind: "find" }
  | { kind: "next-tab" }
  | { kind: "prev-tab" }
  | { kind: "zoom-in" }
  | { kind: "zoom-out" }
  | { kind: "zoom-reset" }
  | { kind: "reopen-closed"; tabId: string }
  | { kind: "select"; index: number };

/** File > Recently Closed items are `reopen-closed:<tab id>`. */
export const REOPEN_CLOSED_PREFIX = "reopen-closed:";

/** The action for a `menu` event's item id, or null for an unknown one. */
export function pageAction(id: unknown): PageAction | null {
  if (typeof id === "string" && id.startsWith(REOPEN_CLOSED_PREFIX) && id.length > REOPEN_CLOSED_PREFIX.length) {
    return { kind: "reopen-closed", tabId: id.slice(REOPEN_CLOSED_PREFIX.length) };
  }
  switch (id) {
    case "new-tab":
    case "close-tab":
    case "reopen-closed-tab":
    case "settings":
    case "next-tab":
    case "prev-tab":
    case "zoom-in":
    case "zoom-out":
    case "zoom-reset":
      return { kind: id };
    default:
      return null;
  }
}

/**
 * A key press the page acts on, or null (`lookupShortcut` has the table).
 * On macOS the native menu takes new / close / reopen tab, new window,
 * Settings, the bracket tab keys and the zoom keys, and sends a `menu`
 * event; the page acts on the rest. Elsewhere there is no native menu, so
 * the page acts on every shortcut.
 */
export function keyAction(event: KeyEvent, platform: Platform): PageAction | null {
  const binding = lookupShortcut(event, platform);
  if (!binding || (platform === "mac" && binding.menu)) return null;
  return binding.action;
}
