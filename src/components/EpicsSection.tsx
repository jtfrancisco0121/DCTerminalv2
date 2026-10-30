import { useCallback, useEffect, useState } from "react";
import { epicsOverview, epicStartWave, type Epic } from "../run/dispatch";
import { errorText } from "./InlineNotice";

const POLL_MS = 5000;

/** The Runs view's Epics: a card per wave (done, building, ready, waiting, with the reason) and Start wave on a ready one. */
export function EpicsSection() {
  const [epics, setEpics] = useState<Epic[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const load = useCallback(
    () =>
      epicsOverview().then(
        (e) => setEpics(e),
        (err: unknown) => setError(errorText(err)),
      ),
    [],
  );
  useEffect(() => {
    void load();
    const timer = window.setInterval(() => void load(), POLL_MS);
    return () => window.clearInterval(timer);
  }, [load]);
  if (error) return <p className="error">{error}</p>;
  if (!epics || epics.length === 0) return null;
  return (
    <section className="runs-epics" aria-label="Epics" data-testid="epics">
      <h3>Epics</h3>
      {epics.map((epic) => (
        <div key={epic.runDir} data-testid={`epic-${epic.runId}`}>
          <p className="hint">{epic.runId}</p>
          {epic.problem && (
            <p className="hint warn" data-testid={`epic-problem-${epic.runId}`}>
              {epic.problem}. No wave starts until it is fixed.
            </p>
          )}
          <ul className="wave-cards">
            {epic.cards.map((card) => (
              <li key={card.n} data-testid={`wave-${epic.runId}-${card.n}`} className={`wave-card wave-${card.state}`}>
                <strong>
                  Wave {card.n}: {card.title}
                </strong>{" "}
                <span className="hint">
                  {card.state}
                  {card.reason ? ` · ${card.reason}` : ""} · {card.ticketIds.length} tickets
                </span>
                {card.state === "ready" && (
                  <button
                    type="button"
                    className="link-button"
                    onClick={() =>
                      epicStartWave(epic.runDir, card.n).then(
                        () => load(),
                        (err: unknown) => setError(errorText(err)),
                      )
                    }
                  >
                    Start wave
                  </button>
                )}
              </li>
            ))}
          </ul>
        </div>
      ))}
    </section>
  );
}
