import type { TabKind } from "../bridge";
import { lookupShortcut } from "../keyTable";

export type TerminalKeyEvent = {
  type: string;
  key: string;
  ctrlKey: boolean;
  metaKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
  isComposing?: boolean;
};

/** ESC then CR: the new-line sequence Claude Code's /terminal-setup installs. */
export const SHIFT_ENTER_SEQUENCE = "\x1b\r";

/**
 * Only Claude Code terminals get the ESC+CR rewrite. A plain shell keeps
 * xterm's CR, so Shift+Enter still runs the command there.
 */
export function shiftEnterSendsNewline(kind: TabKind): boolean {
  return kind === "claude";
}

/**
 * True for Shift+Enter alone. xterm would send a bare CR, which submits the
 * prompt. The app sends ESC+CR instead, so Claude Code inserts a new line.
 */
export function isShiftEnter(event: TerminalKeyEvent): boolean {
  return (
    event.key === "Enter" &&
    event.shiftKey &&
    !event.ctrlKey &&
    !event.metaKey &&
    !event.altKey &&
    !event.isComposing
  );
}

export type Platform = "mac" | "windows" | "linux";

/** Cmd+Shift+E (Ctrl+Shift+E elsewhere): toggles the prompt scratchpad. */
export function isScratchpadShortcut(event: TerminalKeyEvent, platform: Platform): boolean {
  const mod = platform === "mac" ? event.metaKey : event.ctrlKey;
  return event.type === "keydown" && event.key.toLowerCase() === "e" && event.shiftKey && mod && !event.altKey && !(platform === "mac" && event.ctrlKey);
}

/** The palette's "Toggle prompt scratchpad" asks the active pane through this window event. */
export const SCRATCHPAD_EVENT = "dcv2:toggle-scratchpad";

/** What the file-link "reveal" option is called here (the stored setting id stays `finder`). */
export function revealLabel(platform: Platform): string {
  if (platform === "mac") return "Finder (reveal)";
  if (platform === "windows") return "Explorer (reveal)";
  return "File manager (reveal)";
}

export function detectPlatform(uaPlatform?: string): Platform {
  const value = (uaPlatform ?? "").toLowerCase();
  if (value.includes("mac")) return "mac";
  if (value.includes("win")) return "windows";
  return "linux";
}

/**
 * Keys xterm must leave to the app: every shortcut in `keyTable` (new /
 * close / reopen tab, a window, Settings, Find, tabs 1..9, next / previous
 * tab, font zoom). On macOS the native menu takes most of these before the
 * page sees them; this keeps xterm from also sending them to the program.
 * Copy and paste stay with xterm's own handling.
 */
export function isAppShortcut(event: TerminalKeyEvent, platform: Platform): boolean {
  return event.type === "keydown" && lookupShortcut(event, platform) !== null;
}
