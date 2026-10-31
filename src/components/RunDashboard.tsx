import { useEffect, useRef, useState } from "react";
import { focusTab, type ActiveRun } from "../bridge";
import { prStateText, prStateTone, stageText } from "../run/words";
import { reportError } from "../uiErrors";
import { Pill } from "./Pill";
import { Stepper } from "./Stepper";

/** The waiting minutes are redrawn this often; the redraw calls no command. */
export const CLOCK_MS = 60_000;

type Props = {
  /** Every run tab of every window (finished ones too); the page shows the unfinished. */
  runs: ReadonlyArray<ActiveRun>;
  onClose: () => void;
};

/** Whole minutes since an ISO time; null for a missing or unreadable one. */
export function minutesSince(since: string | null | undefined, now: number): number | null {
  if (!since) return null;
  const at = new Date(since).getTime();
  return Number.isNaN(at) ? null : Math.max(0, Math.floor((now - at) / 60_000));
}

/** Mission control: a card per unfinished run tab of every window. */
export function RunDashboard({ runs, onClose }: Props) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), CLOCK_MS);
    return () => window.clearInterval(timer);
  }, []);

  // The page takes the keyboard from a terminal underneath (Escape must not reach it).
  const page = useRef<HTMLElement>(null);
  useEffect(() => page.current?.focus(), []);

  const open = runs.filter((run) => run.stage !== "done" && run.stage !== "failed");

  return (
    <section className="page" role="region" aria-label="Run dashboard" tabIndex={-1} ref={page}>
      <header className="page-header">
        <h2>Run dashboard</h2>
        <button type="button" className="secondary-button" onClick={onClose}>
          Close
        </button>
      </header>
      {open.length === 0 && <p className="hint">No run is in progress.</p>}
      <ul className="run-cards">
        {open.map((run) => {
          const waited = run.waitingFor ? minutesSince(run.waitingSince, now) : null;
          return (
            <li key={run.tabId} className="run-card" data-testid={`card-${run.tabId}`}>
              <div className="run-card-head">
                <strong>{run.label}</strong>
                <span className="hint"> — {stageText(run.stage)}</span>
                {run.waitingFor && (
                  <span className="active-flag active-flag-wait">
                    {" "}
                    waiting for you{waited != null ? ` · ${waited} min` : ""}
                  </span>
                )}
                {run.stalled && <span className="active-flag active-flag-stall"> no progress</span>}
              </div>
              <Stepper
                stage={run.stage}
                waitingFor={run.waitingFor}
                prs={{ done: run.prsDone, total: run.prsTotal }}
                mode={run.runMode}
              />
              {(run.prs ?? []).length > 0 && (
                <div className="run-card-prs">
                  {(run.prs ?? []).map((pr) => (
                    <Pill key={pr.id} tone={prStateTone(pr.state)}>
                      {pr.id}: {prStateText(pr.state)}
                    </Pill>
                  ))}
                </div>
              )}
              <div className="run-card-actions">
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
              </div>
            </li>
          );
        })}
      </ul>
    </section>
  );
}
