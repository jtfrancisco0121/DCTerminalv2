/**
 * Finding file paths in one line of terminal output (AD-18). A pure matcher: it returns candidates with their
 * column range; whether they exist is the backend's answer (`paths_exist`), so a word that only looks like a
 * file (a domain, a version number) is never linked.
 */
import { baseName } from "../paths";

export type FileMatch = {
  /** The path as printed, without the `:line:col`. */
  path: string;
  line?: number;
  col?: number;
  /** Zero-based range of the whole link text in the line (`end` exclusive). */
  start: number;
  end: number;
};

const TOOL_CALL = /\b(?:Write|Read|Edit|MultiEdit|Update|NotebookEdit|Create)\(\s*(["'`]?)([^()\n]+?)\1\s*(?:,|\))/g;
const QUOTED = /(["'`])((?:~\/|\.{1,2}\/|\/)?[^"'`\n]*?[\w-]\.[A-Za-z][A-Za-z0-9]{0,9}|(?:~\/|\.{1,2}\/|\/)[^"'`\n]+)\1/g;
const PLAIN = /(?<![\w./~@:-])((?:~\/|\.{1,2}\/|\/)?(?:[\w@+-]+\/)*[\w@+-]*[\w+-]\.[A-Za-z][A-Za-z0-9]{0,9}(?!\w)|(?:~\/|\.{1,2}\/|\/)(?:[\w@+.-]+\/)+[\w@+.-]+)(?::(\d+)(?::(\d+))?)?/g;
const SUFFIX = /^:(\d+)(?::(\d+))?/;
/** A Windows drive path: `C:\a\b.rs` or `C:/a/b.rs`, optionally with `:line` or `:line:col` after it. */
const WINDOWS = /(?<![\w./~@:\\-])([A-Za-z]:[\\/](?:[\w@+.-]+[\\/])*[\w@+.-]*[\w+-]\.[A-Za-z][A-Za-z0-9]{0,9})(?!\w)(?::(\d+)(?::(\d+))?)?/g;

function plausible(path: string): boolean {
  if (!path || path.length > 400 || /\s{2,}/.test(path) || /^\w+:\/\//.test(path)) return false;
  // 1.2.3, v0.7.0 and 3.14 are numbers; a path ends in a name with a letter in it.
  if (path.includes("@") && !path.includes("/")) return false; // an email address
  const last = baseName(path);
  if (/^v?\d+(\.\d+)+$/.test(last)) return false;
  return /[A-Za-z]/.test(last);
}

/** Every path-like piece of `text`, in line order, without overlaps. */
export function findFilePaths(text: string): FileMatch[] {
  const found: FileMatch[] = [];
  const taken = (start: number, end: number) => found.some((m) => start < m.end && end > m.start);
  const add = (path: string, start: number, end: number, rest: string) => {
    const suffix = SUFFIX.exec(rest);
    const stop = suffix ? end + suffix[0].length : end;
    if (!plausible(path) || taken(start, stop)) return;
    found.push({ path, start, end: stop, line: suffix ? Number(suffix[1]) : undefined, col: suffix?.[2] ? Number(suffix[2]) : undefined });
  };
  for (const m of text.matchAll(WINDOWS)) {
    const path = m[1] ?? "";
    const start = m.index!;
    const end = start + path.length;
    add(path, start, end, text.slice(end));
  }
  for (const m of text.matchAll(TOOL_CALL)) {
    const path = (m[2] ?? "").trim();
    const start = m.index! + m[0].indexOf(path);
    add(path, start, start + path.length, text.slice(start + path.length + (m[1]?.length ?? 0)));
  }
  for (const m of text.matchAll(QUOTED)) {
    const path = m[2] ?? "";
    if (!/[/.]/.test(path)) continue;
    const start = m.index! + 1;
    add(path, start, start + path.length, "");
  }
  for (const m of text.matchAll(PLAIN)) {
    const path = m[1] ?? "";
    const start = m.index!;
    if (text[start - 1] === "@" || /^(?:www\.)/i.test(path)) continue;
    const end = start + path.length;
    if (m[2]) add(path, start, end, text.slice(end));
    else add(path, start, end, "");
  }
  return found.sort((a, b) => a.start - b.start);
}
