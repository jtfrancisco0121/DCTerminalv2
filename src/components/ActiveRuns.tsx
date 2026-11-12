import { useEffect, useState } from "react";
import { activeRuns, focusTab, type ActiveRun } from "../bridge";
import { evalFinished, NO_RUN_TEXT, prFraction, stageText } from "../run/words";
import { RUN_PAGES, type RunPageId } from "../runPages";
import { Stepper } from "./Stepper";
import { BatchOverview } from "./BatchOverview";
import { reportError } from "../uiErrors";
import { RUN_STATE_EVENT, TAB_STATE_EVENT, usePushRefresh } from "../run/events";

/** The list is read again this often while the popover is open (at most). */
export const POLL_MS = 3000;

type Props = {
  onClose: () => void;
  /** A footer link: open that run page (the popover closes itself). */
  onOpenPage: (id: RunPageId) => void;
  /** An eval group's "Results" link in the batch overview. */
  onEvalResults?: (group: string) => void;
};

/** Every unfinished run tab of every window, with its stage, flags and PR progress. */
export function ActiveRuns({ onClose, onOpenPage, onEvalResults }: Props) {
  const [runs, setRuns] = useState<ActiveRun[] | null>(null);

  // Events from the detector; polls at POLL_MS when none arrive.
  usePushRefresh([RUN_STATE_EVENT, TAB_STATE_EVENT], () =>
    activeRuns().then(setRuns, (err: unknown) => reportError("Active runs", err, { once: true })),
    { fastMs: POLL_MS },
  );

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  return (
    <div className="active-runs" role="dialog" aria-label="Active runs">
      <h3>Active runs</h3>
      {runs === null && <p className="hint">Looking…</p>}
      {runs && runs.length === 0 && <p className="hint">{NO_RUN_TEXT}</p>}
      <ul>
        {runs?.map((run) => (
          <li key={run.tabId} data-testid={`active-${run.tabId}`}>
            <div>
              <strong>{run.label}</strong>
              <span className="hint">
                {" "}
                — {stageText(run.stage, run.eval)}
                {run.prsTotal > 0 ? ` · ${prFraction(run.prsDone, run.prsTotal)}` : ""}
              </span>
              {run.waitingFor && <span className="active-flag active-flag-wait"> waiting for you</span>}
              {!run.waitingFor && run.activity === "needs-you" && (
                <span className="active-flag active-flag-wait"> needs you</span>
              )}
              {run.stalled && <span className="active-flag active-flag-stall"> no progress</span>}
              <Stepper
                stage={run.stage}
                waitingFor={run.waitingFor}
                prs={{ done: run.prsDone, total: run.prsTotal }}
                mode={run.eval ? "eval" : run.runMode}
                finished={evalFinished(run)}
                variant="compact"
              />
            </div>
            <button
              type="button"
              className="link-button"
              onClick={() => {
                focusTab(run.tabId).catch((err: unknown) => reportError("Go to tab", err, { notify: true }));
                onClose();
              }}
            >
              Go to tab
            </button>
          </li>
        ))}
      </ul>
      <BatchOverview compact onEvalResults={onEvalResults} />
      <div className="active-runs-footer">
        {RUN_PAGES.filter((page) => page.inPopover).map((page) => (
          <button
            key={page.id}
            type="button"
            className="link-button"
            onClick={() => {
              onOpenPage(page.id);
              onClose();
            }}
          >
            {page.short}
          </button>
        ))}
        <button type="button" className="link-button" onClick={onClose}>
          Close
        </button>
      </div>
    </div>
  );
}
