import { openUrl } from "@tauri-apps/plugin-opener";
import { useCallback, useEffect, useState } from "react";
import { batchOverview, batchRemoveRun, type Batch } from "../run/dispatch";
import { safeUrl, stageText } from "../run/words";
import { errorText } from "./InlineNotice";

const POLL_MS = 3000;

const ciText = (ci: string | null) =>
  ci === "pass" ? "CI passing" : ci === "fail" ? "CI failing" : ci === "pending" ? "CI running" : ci === "unknown" ? "CI unknown" : "";

/** One row per run of each batch: stage, what it waits for, its PR and CI; a waiting-for-you badge; a filter by batch. */
export function BatchOverview({ compact = false }: { compact?: boolean }) {
  const [batches, setBatches] = useState<Batch[] | null>(null);
  const [only, setOnly] = useState("");
  const [error, setError] = useState<string | null>(null);
  const load = useCallback(
    () =>
      batchOverview().then(
        (b) => setBatches(b),
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
  if (!batches || batches.length === 0) return null;
  const shown = batches.filter((b) => !only || b.id === only);
  const waiting = shown.reduce((n, b) => n + b.waitingForYou, 0);
  return (
    <section className="batch-overview" aria-label="Batches" data-testid="batches">
      <h3>
        Batches{" "}
        {waiting > 0 && (
          <span className="active-flag active-flag-wait" data-testid="batch-waiting">
            {waiting} waiting for you
          </span>
        )}
      </h3>
      {!compact && batches.length > 1 && (
        <select aria-label="Batch" className="text-input text-input-narrow" value={only} onChange={(event) => setOnly(event.target.value)}>
          <option value="">All batches</option>
          {batches.map((b) => (
            <option key={b.id} value={b.id}>
              {b.id}
            </option>
          ))}
        </select>
      )}
      {shown.map((batch) => (
        <div key={batch.id} className="batch" data-testid={`batch-${batch.id}`}>
          <p className="hint">{batch.id}</p>
          <ul>
            {batch.rows.map((row) => {
              const url = safeUrl(row.prUrl);
              return (
                <li key={row.entryId} data-testid={`batch-row-${row.entryId}`} style={row.removed ? { textDecoration: "line-through", opacity: 0.6 } : undefined}>
                  <strong>{row.ticketIds.join(", ") || row.label}</strong>{" "}
                  <span className="hint">
                    {row.stage === "queued" || row.stage === "starting" ? row.stage : stageText(row.stage)}
                    {row.removed ? " · removed from the batch" : ""}
                  </span>
                  {!row.removed && row.waitingFor && <span className="active-flag active-flag-wait"> waiting for you ({row.waitingFor === "ship-approval" ? "ship" : "plan"})</span>}
                  {url && (
                    <button type="button" className="link-button" onClick={() => void openUrl(url)}>
                      PR
                    </button>
                  )}
                  {row.ci && <span className="hint"> {ciText(row.ci)}</span>}
                  {!row.removed && (
                    <button
                      type="button"
                      className="link-button"
                      onClick={() =>
                        batchRemoveRun(batch.id, row.entryId).then(
                          () => load(),
                          (err: unknown) => setError(errorText(err)),
                        )
                      }
                    >
                      Remove from batch
                    </button>
                  )}
                </li>
              );
            })}
          </ul>
        </div>
      ))}
    </section>
  );
}
