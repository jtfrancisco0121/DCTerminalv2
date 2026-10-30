import { useCallback, useEffect, useState } from "react";
import {
  dispatchCancel,
  dispatchMove,
  dispatchOverview,
  dispatchRelease,
  dispatchStartNow,
  queueStateText,
  type DispatchOverview,
} from "../run/dispatch";
import { stageText } from "../run/words";
import { errorText } from "./InlineNotice";

const POLL_MS = 3000;

/** The Runs view's Queue (entries with their reasons, up/down/start now/cancel) and the runs that hold slots. */
export function QueueSection() {
  const [overview, setOverview] = useState<DispatchOverview | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [confirm, setConfirm] = useState<string | null>(null);
  const load = useCallback(
    () =>
      dispatchOverview().then(
        (o) => setOverview(o),
        (err: unknown) => setError(errorText(err)),
      ),
    [],
  );
  useEffect(() => {
    void load();
    const timer = window.setInterval(() => void load(), POLL_MS);
    return () => window.clearInterval(timer);
  }, [load]);
  const act = (work: Promise<unknown>) =>
    work.then(
      () => load(),
      (err: unknown) => setError(errorText(err)),
    );
  if (!overview) return error ? <p className="error">{error}</p> : null;
  if (overview.queue.length === 0 && overview.holders.length === 0) return null;
  return (
    <section className="runs-queue" aria-label="Queue" data-testid="queue">
      {error && <p className="error">{error}</p>}
      <h3>
        Queue <span className="hint">({overview.holders.length} of {overview.cap} slots in use)</span>
      </h3>
      {overview.queue.length === 0 ? (
        <p className="hint">Nothing is waiting.</p>
      ) : (
        <ol className="runs-queue-list">
          {overview.queue.map((entry, at) => (
            <li key={entry.id} data-testid={`queue-${entry.id}`}>
              <strong>{entry.label}</strong> <span className="hint">{queueStateText(entry)}</span>
              <span className="runs-queue-actions">
                <button type="button" className="link-button" disabled={at === 0 || entry.state !== "queued"} onClick={() => void act(dispatchMove(entry.id, true))}>
                  Up
                </button>
                <button
                  type="button"
                  className="link-button"
                  disabled={at === overview.queue.length - 1 || entry.state !== "queued"}
                  onClick={() => void act(dispatchMove(entry.id, false))}
                >
                  Down
                </button>
                <button type="button" className="link-button" disabled={entry.state !== "queued"} onClick={() => setConfirm(entry.id)}>
                  Start now
                </button>
                <button type="button" className="link-button" disabled={entry.state !== "queued"} onClick={() => void act(dispatchCancel(entry.id))}>
                  Cancel
                </button>
              </span>
              {confirm === entry.id && (
                <p className="hint warn" role="alert">
                  Start it now, past the limit of {overview.cap}?{" "}
                  <button
                    type="button"
                    className="link-button"
                    onClick={() => {
                      setConfirm(null);
                      void act(dispatchStartNow(entry.id));
                    }}
                  >
                    Yes, start now
                  </button>{" "}
                  <button type="button" className="link-button" onClick={() => setConfirm(null)}>
                    No
                  </button>
                </p>
              )}
            </li>
          ))}
        </ol>
      )}
      {overview.usageHoldPercent !== null && overview.queue.some((e) => e.reason?.includes("usage")) && (
        <p className="hint">Starts are held while the five-hour usage is at or past {overview.usageHoldPercent}%.</p>
      )}
      <h4>Holding slots</h4>
      {overview.holders.length === 0 ? (
        <p className="hint">No run holds a slot.</p>
      ) : (
        <ul className="runs-holders">
          {overview.holders.map((h) => (
            <li key={h.runDir} data-testid={`holder-${h.runId}`}>
              <strong>{h.runId}</strong> {stageText(h.stage)} <span className="hint">counts because: {h.why}</span>
              <button type="button" className="link-button" onClick={() => void act(dispatchRelease(h.runDir))}>
                Release slot
              </button>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
