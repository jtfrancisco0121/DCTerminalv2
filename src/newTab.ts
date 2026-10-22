/** The new-tab form's starting values. */

import type { AppStateSnapshot, TabKind } from "./bridge";

export type NewTabDraft = { cwd: string; kind: TabKind };

/**
 * The active tab's folder (else the newest tab's, else the last folder this
 * window used), and Claude unless the active tab is a shell.
 */
export function newTabDefaults(state: AppStateSnapshot | null, lastFolder = ""): NewTabDraft {
  const tabs = state?.tabs ?? [];
  const active = tabs.find((tab) => tab.id === state?.activeTabId);
  const from = active ?? tabs[tabs.length - 1];
  return { cwd: from?.cwd ?? lastFolder, kind: active?.kind ?? "claude" };
}
