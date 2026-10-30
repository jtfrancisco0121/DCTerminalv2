/** Advice on a draft prompt. Hints never block Send. */
export const LONG_DRAFT_CHARS = 4000;

const CRITERIA = /\b(acceptance criteria|accept(?:s|ed)? when|done when|must (?:pass|work|return)|should (?:pass|return|show)|so that|expected:?)\b/i;
// A path (a/b, ./x), a file name with an extension, or a CamelCase/snake_case identifier ending in a file-like suffix.
const FILE_OR_AREA = /(?:^|[\s"'`(])(?:\.{0,2}\/)?[\w.-]+\/[\w./-]+|\b[\w-]+\.(?:ts|tsx|js|jsx|rs|py|go|java|md|json|toml|sh|css|html|yml|yaml)\b|\b(?:src|app|lib|api|server|web|plugin|docs|tests?)\b/i;

export function draftHints(text: string): string[] {
  if (!text.trim()) return [];
  const hints: string[] = [];
  if (!CRITERIA.test(text)) hints.push("No acceptance criteria: say how you will know it is done.");
  if (!FILE_OR_AREA.test(text)) hints.push("It names no file or area: point at where the change belongs.");
  if (text.length > LONG_DRAFT_CHARS) hints.push("Very long: consider splitting it into steps.");
  return hints;
}

/**
 * Send is allowed for a one-line draft, or for any draft when the terminal is
 * in bracketed paste mode (a newline is then pasted, not typed as Enter).
 * Any `\n`, a trailing one included, counts as a line break.
 */
export function sendBlockReason(text: string, bracketedPaste: boolean): string | null {
  if (!text.trim()) return "Write something to send.";
  if (text.includes("\n") && !bracketedPaste) {
    return "This terminal is not in paste mode, so line breaks would run as separate commands. Remove the line breaks to send.";
  }
  return null;
}
