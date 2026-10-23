/**
 * Reads what you paste into "Tickets from ClickUp" (C2): ClickUp task links,
 * list links and bare task ids, mixed, one or more per line. The parser is
 * pure so the box can say which line it could not read before anything is
 * fetched; the core checks every id again before it builds a request.
 *
 * Accepted forms:
 * - `https://app.clickup.com/t/<taskId>`: a task by ClickUp's own id.
 * - `https://app.clickup.com/t/<teamId>/<customId>`: a custom task id, in
 *   that workspace.
 * - `https://app.clickup.com/<teamId>/v/li/<listId>`: a list.
 * - `https://app.clickup.com/<teamId>/v/l/6-<listId>-<n>`: a list's view,
 *   as ClickUp's address bar shows it (`6` marks a list view).
 * - Any of these links inside a Markdown link `[text](url)` or a Slack
 *   link `<url|text>`, as a copy from a doc or a chat gives them.
 * - `86b1aa001` or `#86b1aa001`: a task id (five or more letters and
 *   digits, with a digit). With a dash (`QA-128`) it is a custom task id
 *   in the window account's workspace.
 */

/** One ref, as `clickup_intake_preview` takes it. */
export type IntakeRef = {
  kind: "task" | "list";
  id: string;
  /** A custom task id's workspace, from its link. */
  teamId?: string;
  /** A custom task id (`QA-128`) rather than ClickUp's own. */
  custom?: boolean;
  /** The line it was on (from 1), for errors. */
  line: number;
};

export type RefProblem = { line: number; message: string };

export type ParsedRefs = { refs: IntakeRef[]; problems: RefProblem[] };

/** The most refs one paste may hold (the core's `MAX_REFS`). */
export const MAX_INTAKE_REFS = 50;

const ID = /^[A-Za-z0-9_-]{1,64}$/;
/** ClickUp's own task ids: letters and digits, with a digit, so a word is not one. */
const NATIVE_ID = /^(?=[A-Za-z]*\d)[A-Za-z0-9]{5,64}$/;
const CLICKUP_HOSTS = new Set(["app.clickup.com", "sharing.clickup.com"]);

function notALink(line: number): RefProblem {
  return { line, message: `Line ${line} is not a ClickUp task or list link.` };
}

/** One token of a line as a ref, or null when it is none. */
function readToken(token: string, line: number): IntakeRef | null {
  if (/^https?:\/\//i.test(token)) return readUrl(token, line);
  const bare = token.replace(/^#/, "");
  if (NATIVE_ID.test(bare)) return { kind: "task", id: bare, line };
  if (ID.test(bare) && bare.includes("-") && /[A-Za-z]/.test(bare) && /\d/.test(bare)) {
    return { kind: "task", id: bare, custom: true, line };
  }
  return null;
}

function readUrl(text: string, line: number): IntakeRef | null {
  let url: URL;
  try {
    url = new URL(text);
  } catch {
    return null;
  }
  if (!CLICKUP_HOSTS.has(url.hostname.toLowerCase())) return null;
  const parts = url.pathname.split("/").filter(Boolean);
  const ok = (id: string | undefined) => !!id && ID.test(id);
  const t = parts.indexOf("t");
  if (t >= 0) {
    const [first, second] = parts.slice(t + 1);
    if (second !== undefined) {
      return ok(first) && ok(second) ? { kind: "task", id: second, teamId: first, custom: true, line } : null;
    }
    return ok(first) ? { kind: "task", id: first, line } : null;
  }
  const v = parts.indexOf("v");
  if (v >= 0) {
    const [kind, id] = parts.slice(v + 1);
    if (kind === "li" && ok(id)) return { kind: "list", id, line };
    const view = kind === "l" ? /^6-([A-Za-z0-9]+)-\d+$/.exec(id ?? "") : null;
    if (view) return { kind: "list", id: view[1], line };
  }
  return null;
}

/** `[text](url)` and `<url|text>` as their url, so the text is not read as refs. */
function unwrapLinks(line: string): string {
  return line
    .replace(/\[[^\]]*\]\((https?:\/\/[^)\s]+)\)/g, " $1 ")
    .replace(/<(https?:\/\/[^|>\s]+)\|[^>]*>/g, " $1 ");
}

function sameRef(a: IntakeRef, b: IntakeRef): boolean {
  return a.kind === b.kind && a.id === b.id && (a.teamId ?? "") === (b.teamId ?? "");
}

/**
 * Every ref in `text`, in order and once each, and a problem for each line
 * that holds something that is not one. Commas and spaces separate refs on
 * a line; blank lines are skipped.
 */
export function parseIntakeRefs(text: string): ParsedRefs {
  const refs: IntakeRef[] = [];
  const problems: RefProblem[] = [];
  text.split(/\r?\n/).forEach((raw, index) => {
    const line = index + 1;
    const tokens = unwrapLinks(raw)
      .split(/[\s,]+/)
      .map((token) => token.trim().replace(/[)>\].;]+$/, "").replace(/^[<(]+/, ""))
      .filter(Boolean);
    let bad = false;
    for (const token of tokens) {
      const ref = readToken(token, line);
      if (!ref) {
        bad = true;
        continue;
      }
      if (!refs.some((seen) => sameRef(seen, ref))) refs.push(ref);
    }
    if (bad) problems.push(notALink(line));
  });
  if (refs.length > MAX_INTAKE_REFS) {
    problems.push({
      line: refs[MAX_INTAKE_REFS].line,
      message: `Paste at most ${MAX_INTAKE_REFS} links at once; line ${refs[MAX_INTAKE_REFS].line} is one too many.`,
    });
    refs.length = MAX_INTAKE_REFS;
  }
  return { refs, problems };
}
