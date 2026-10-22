/**
 * How a tab's terminal starts the first time it is shown in this page: a
 * Claude tab that ran before resumes its session when that session can
 * still be resumed (after a restart, or a reopened tab), else it starts
 * fresh. A shell always starts fresh. A tab whose folder is gone does not
 * start at all, nor does a Claude tab whose window's account was removed.
 */

import type { TabSummary } from "../bridge";

/** Same words as the backend's refusal (`settings_store::ACCOUNT_REMOVED`). */
export const ACCOUNT_REMOVED = "This window's account was removed. Pick an account.";

/**
 * Why a Claude tab cannot start in this window: its account is no longer in
 * Settings. It never starts under another account in its place (that would
 * also save the other account's session over the tab's own).
 */
export function accountProblem(tab: Pick<TabSummary, "kind">, accountMissing: boolean): string | null {
  return tab.kind === "claude" && accountMissing ? ACCOUNT_REMOVED : null;
}

/** Why the tab's terminal cannot start, or null when its folder is fine. */
export function folderProblem(tab: Pick<TabSummary, "cwd" | "folderStatus">): string | null {
  switch (tab.folderStatus) {
    case "ok":
      return null;
    case "missing":
      return `This tab's folder was not found (it may have been moved or deleted): ${tab.cwd}`;
    case "not-a-directory":
      return `This tab's folder is a file, not a folder: ${tab.cwd}`;
    case "unreadable":
      return `This tab's folder cannot be read: ${tab.cwd}`;
    default:
      return "This tab has no folder.";
  }
}

/**
 * Whether the first open resumes. `resumeId` looks up the session the tab
 * can resume now (`terminal_resume_id`); it is asked only for a Claude tab
 * that started a session before. A failed lookup starts fresh.
 */
export async function firstOpenResumes(
  tab: Pick<TabSummary, "id" | "kind" | "hasSession">,
  resumeId: (tabId: string) => Promise<string | null>,
): Promise<boolean> {
  if (tab.kind !== "claude" || !tab.hasSession) return false;
  try {
    return (await resumeId(tab.id)) != null;
  } catch {
    return false;
  }
}
