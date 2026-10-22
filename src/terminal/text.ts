/** Plain-text helpers for terminal hand-off and search. */

export const TERMINAL_TAIL_LINES = 200;

/** Strip CSI, OSC, and other non-printing controls. Newlines stay. */
export function stripAnsi(input: string): string {
  return input
    .replace(/\u001B\][\s\S]*?(?:\u0007|\u001B\\)/g, "")
    .replace(/\u001B\[[0-?]*[ -/]*[@-~]/g, "")
    .replace(/\u001B[@-_]/g, "")
    .replace(/[\u0000-\u0008\u000B\u000C\u000E-\u001A\u001C-\u001F\u007F]/g, "");
}

/** `*.md` file names a terminal printed, each once, the latest mention last. */
export function planFileMentions(text: string): string[] {
  const names: string[] = [];
  for (const match of stripAnsi(text).matchAll(/([A-Za-z0-9_][A-Za-z0-9_.-]*\.md)(?![A-Za-z0-9_-])/g)) {
    const at = names.indexOf(match[1]);
    if (at >= 0) names.splice(at, 1);
    names.push(match[1]);
  }
  return names;
}

export function lastLines(text: string, count = TERMINAL_TAIL_LINES): string {
  const clean = stripAnsi(text).replace(/\r\n/g, "\n").replace(/\r/g, "\n");
  const lines = clean.split("\n");
  const start = Math.max(0, lines.length - Math.max(count, 0));
  return lines.slice(start).join("\n").trim();
}
