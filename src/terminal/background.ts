/**
 * Opens a tab's terminal without showing it. A run started by the
 * dispatcher (queue, wave) has a tab nobody selected, so no view mounts and
 * nothing would start `claude`. The terminal is parked like any hidden tab:
 * its xterm drains the output and answers queries, and when the tab is shown
 * later the view's own `ifIdle` open finds the session and attaches.
 */

import { terminalResumeId, type TabSummary } from "../bridge";
import { reportError } from "../uiErrors";
import { dropLivePty, isPtyOpening, openLivePty, ptyRunning } from "./live";
import { accountProblem, firstOpenResumes, folderProblem } from "./openPlan";
import { ensureParkedTerminal, measuredTerminalSize } from "./park";

/** Never throws; repeated calls for one tab start at most one program. */
export function openInBackground(
  tab: TabSummary,
  fontSize: number,
  accountMissing: boolean,
  resumeId: (tabId: string) => Promise<string | null> = terminalResumeId,
): void {
  if (folderProblem(tab) || accountProblem(tab, accountMissing)) return;
  if (ptyRunning(tab.id) || isPtyOpening(tab.id)) return;
  ensureParkedTerminal(tab.id, fontSize);
  void openLivePty(tab.id, {
    ifIdle: true,
    before: () => firstOpenResumes(tab, resumeId),
    // No mounted slot is needed: 80x24 until a view fits it.
    start: (resume) => ({ ...measuredTerminalSize(), resume }),
    onError: (message) => {
      reportError("Start run tab", message, { notify: true, once: `bg-open:${tab.id}` });
      dropLivePty(tab.id);
    },
  });
}
