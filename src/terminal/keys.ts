import type { TabKind } from "../bridge";

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

export function detectPlatform(uaPlatform?: string): Platform {
  const value = (uaPlatform ?? "").toLowerCase();
  if (value.includes("mac")) return "mac";
  if (value.includes("win")) return "windows";
  return "linux";
}

/**
 * Keys xterm must leave to the app: the app's own shortcuts (Cmd on macOS,
 * Ctrl+Shift elsewhere) for new / close / reopen tab, a window, Settings,
 * and Find. On macOS the native menu takes most of these before the page
 * sees them; this keeps xterm from also sending them to the program.
 * Copy and paste stay with xterm's own handling.
 */
export function isAppShortcut(event: TerminalKeyEvent, platform: Platform): boolean {
  if (event.type !== "keydown") return false;
  const key = event.key.toLowerCase();
  const command =
    platform === "mac"
      ? event.metaKey && !event.ctrlKey && !event.altKey
      : event.ctrlKey && event.shiftKey && !event.metaKey && !event.altKey;
  if (!command) return false;
  return ["t", "w", "n", ",", "f"].includes(key) || /^[1-9]$/.test(key);
}
