/** Paths dropped on a terminal become quoted text, pasted like Cmd+V. */

/** Quote one path for the shell, or for Claude Code's prompt box. */
export function quoteDroppedPath(path: string, windows: boolean): string {
  if (windows) return `"${path.replace(/"/g, "")}"`;
  return `'${path.replace(/'/g, `'\\''`)}'`;
}

/** The text to paste for dropped files: quoted, space-separated, trailing space. */
export function formatDroppedPaths(paths: readonly string[], windows: boolean): string {
  const quoted = paths.filter(Boolean).map((path) => quoteDroppedPath(path, windows));
  return quoted.length > 0 ? `${quoted.join(" ")} ` : "";
}

/**
 * Tauri reports a drop position in physical pixels; `elementFromPoint` wants
 * CSS pixels. `scale` is `window.devicePixelRatio`; 0 or NaN counts as 1.
 */
export function dropPointToCss(position: { x: number; y: number }, scale: number): { x: number; y: number } {
  const factor = Number.isFinite(scale) && scale > 0 ? scale : 1;
  return { x: position.x / factor, y: position.y / factor };
}
