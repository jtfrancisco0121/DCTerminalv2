import { useEffect, useState } from "react";
import { activeRuns, focusTab, type ActiveRun } from "../bridge";
import { stageText } from "../run/words";
import { Stepper } from "./Stepper";
import { BatchOverview } from "./BatchOverview";
import { reportError } from "../uiErrors";
import { RUN_STATE_EVENT, TAB_STATE_EVENT, usePushRefresh } from "../run/events";

/** The list is read again this often while the popover is open (at most). */
export const POLL_MS = 3000;

type Props = {
  onClose: () => void;
  /** The footer link: open the Runs view. */
  onAllRuns: () => void;
};

/** Every unfinished run tab of every window, with its stage, flags and PR progress. */
export function ActiveRuns({ onClose, onAllRuns }: Props) {
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
      {runs && runs.length === 0 && <p className="hint">No run is in progress.</p>}
      <ul>
        {runs?.map((run) => (
          <li key={run.tabId} data-testid={`active-${run.tabId}`}>
            <div>
              <strong>{run.label}</strong>
              <span className="hint">
                {" "}
                — {stageText(run.stage)}
                {run.prsTotal > 0 ? ` · ${run.prsDone}/${run.prsTotal} PRs` : ""}
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
                mode={run.runMode}
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
      <BatchOverview compact />
      <div className="active-runs-footer">
        <button
          type="button"
          className="link-button"
          onClick={() => {
            onAllRuns();
            onClose();
          }}
        >
          All runs
        </button>
        <button type="button" className="link-button" onClick={onClose}>
          Close
        </button>
      </div>
    </div>
  );
}
