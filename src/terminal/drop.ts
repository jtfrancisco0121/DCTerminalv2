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
