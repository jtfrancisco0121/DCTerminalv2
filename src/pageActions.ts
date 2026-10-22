/**
 * What the page does for a menu item (the native macOS menu sends its id)
 * or a shortcut it handles itself.
 */

import type { Platform } from "./terminal/keys";

export type PageAction =
  | { kind: "new-tab" }
  | { kind: "close-tab" }
  | { kind: "reopen-closed-tab" }
  | { kind: "settings" }
  | { kind: "new-window" }
  | { kind: "find" }
  | { kind: "select"; index: number };

/** The action for a `menu` event's item id, or null for an unknown one. */
export function pageAction(id: unknown): PageAction | null {
  switch (id) {
    case "new-tab":
    case "close-tab":
    case "reopen-closed-tab":
    case "settings":
      return { kind: id };
    default:
      return null;
  }
}

/**
 * A key press the page acts on, or null. Find and Cmd+1…9 everywhere; on
 * other platforms (no native menu) also Ctrl+Shift+T/W/N/, for new tab,
 * close tab, new window and Settings. macOS gets those from the menu.
 */
export function keyAction(
  event: Pick<KeyboardEvent, "key" | "metaKey" | "ctrlKey" | "altKey" | "shiftKey">,
  platform: Platform,
): PageAction | null {
  const command =
    platform === "mac"
      ? event.metaKey && !event.ctrlKey && !event.altKey
      : event.ctrlKey && event.shiftKey && !event.metaKey && !event.altKey;
  if (!command) return null;
  const key = event.key.toLowerCase();
  if (key === "f") return { kind: "find" };
  if (/^[1-9]$/.test(key)) return { kind: "select", index: Number(key) - 1 };
  if (platform === "mac") return null;
  if (key === "t") return { kind: "new-tab" };
  if (key === "w") return { kind: "close-tab" };
  if (key === "n") return { kind: "new-window" };
  if (key === ",") return { kind: "settings" };
  return null;
}
