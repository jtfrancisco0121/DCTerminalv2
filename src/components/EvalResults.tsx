import { useCallback, useEffect, useRef, useState } from "react";
import { flightEvalResults } from "../flight/api";
import type { Dist, EvalResults as Results, EvalRunResult, KeptBranch } from "../flight/types";
import { evalCleanUp, evalDeleteBranch, type EvalCleanLine } from "../run/api";
import { evalCheckText, evalDistText, minutesText, outcomeText } from "../run/words";
import { Dialog } from "./Dialog";
import { errorText } from "./InlineNotice";
import { Pill } from "./Pill";

/** How often the page reads the results again while a run is going or being checked. */
export const POLL_MS = 4000;

type Props = {
  /** The eval group (the dispatch batch id). */
  group: string;
  onClose: () => void;
  /** Open a run's Trace (Escape there returns here). */
  onOpenTrace?: (runDir: string) => void;
};

type Measure = {
  key: keyof Results["distributions"];
  label: string;
  value: (run: EvalRunResult) => number | null;
  format: (value: number) => string;
};

const percent = (v: number) => `${Math.round(v * 100)}%`;
const MEASURES: Measure[] = [
  {
    key: "completed",
    label: "Completed",
    value: (r) => (r.completed === null ? null : r.completed ? 1 : 0),
    format: (v) => (v >= 1 ? "yes" : v <= 0 ? "no" : percent(v)),
  },
  { key: "checkPassRate", label: "Check pass rate", value: (r) => r.checkPassRate, format: percent },
  { key: "retries", label: "Retries", value: (r) => r.retries, format: (v) => String(v) },
  { key: "toolFailures", label: "Tool failures", value: (r) => r.toolFailures, format: (v) => String(v) },
  {
    key: "tokens",
    label: "Tokens (in + out)",
    value: (r) => r.tokens,
    format: (v) => Math.round(v).toLocaleString("en-US"),
  },
  { key: "activeMinutes", label: "Active minutes", value: (r) => r.activeMinutes, format: (v) => minutesText(v) },
];

const W = 220;
const PAD = 8;

/** One measure as a strip: a dot per finished run on a line from the smallest to the largest value, and the median. */
function Strip({ measure, dist, runs }: { measure: Measure; dist: Dist; runs: EvalRunResult[] }) {
  const text = `${measure.label}: ${evalDistText(dist, measure.format)}`;
  const values = runs
    .filter((r) => r.status === "finished")
    .map(measure.value)
    .filter((v): v is number => v !== null);
  const lo = dist.min ?? 0;
  const hi = dist.max ?? 0;
  const x = (v: number) => (hi === lo ? W / 2 : PAD + ((v - lo) / (hi - lo)) * (W - 2 * PAD));
  return (
    <li className="eval-strip" data-testid={`strip-${measure.key}`}>
      <span className="eval-strip-label">{measure.label}</span>
      <svg role="img" aria-label={text} width={W} height={26} viewBox={`0 0 ${W} 26`}>
        <title>{text}</title>
        <line x1={PAD} x2={W - PAD} y1={13} y2={13} stroke="currentColor" strokeOpacity={0.3} />
        {dist.min !== null &&
          values.map((v, i) => (
            <circle key={i} cx={x(v)} cy={13} r={4} className="eval-dot" fill="currentColor" fillOpacity={0.55} />
          ))}
        {dist.median !== null && (
          <line x1={x(dist.median)} x2={x(dist.median)} y1={4} y2={22} stroke="currentColor" strokeWidth={2} />
        )}
      </svg>
      <span className="hint" data-testid={`strip-text-${measure.key}`}>
        {evalDistText(dist, measure.format)}
      </span>
    </li>
  );
}

function statusText(run: EvalRunResult): string {
  switch (run.status) {
    case "finished":
      return outcomeText("eval-done");
    case "failed":
      return "Failed";
    case "queued":
      return "Waiting in the queue";
    default:
      return "Running";
  }
}

/**
 * The results of one eval group: the six measures as distributions over the finished runs (running and failed runs
 * are counted apart), the runs with their Checks column, one Clean up for the group and the branches the runs kept,
 * each with a Delete the user confirms. Everything is read from `flight_eval_results`; nothing is computed here.
 */
export function EvalResults({ group, onClose, onOpenTrace }: Props) {
  const [results, setResults] = useState<Results | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [lines, setLines] = useState<EvalCleanLine[] | null>(null);
  const [confirmClean, setConfirmClean] = useState(false);
  const [busy, setBusy] = useState(false);
  const [deleting, setDeleting] = useState<{ runDir: string; branch: KeptBranch } | null>(null);
  const page = useRef<HTMLElement>(null);

  useEffect(() => page.current?.focus(), []);

  const load = useCallback(
    () =>
      flightEvalResults(group).then(
        (r) => {
          setResults(r);
          setError(null);
        },
        (err: unknown) => setError(errorText(err)),
      ),
    [group],
  );
  useEffect(() => {
    void load();
    const timer = window.setInterval(() => void load(), POLL_MS);
    return () => window.clearInterval(timer);
  }, [load]);

  const finishedRuns = results?.runs.filter((r) => r.status === "finished") ?? [];
  const unchecked = finishedRuns.filter((r) => r.checkState === "not checked");

  const cleanUp = async () => {
    setConfirmClean(false);
    setBusy(true);
    try {
      setLines(await evalCleanUp(group));
      setError(null);
    } catch (err) {
      setError(errorText(err));
    } finally {
      setBusy(false);
      void load();
    }
  };

  const deleteBranch = async () => {
    if (!deleting) return;
    const { runDir, branch } = deleting;
    setDeleting(null);
    try {
      await evalDeleteBranch(runDir, branch.prId);
      setError(null);
    } catch (err) {
      setError(errorText(err));
    }
    void load();
  };

  const kept = (results?.runs ?? []).flatMap((run) => run.keptBranches.map((branch) => ({ run, branch })));

  return (
    <section className="page eval-results" role="region" aria-label="Eval results" tabIndex={-1} ref={page}>
      <header className="page-header">
        <h2>Eval results</h2>
        <button type="button" className="secondary-button" onClick={onClose}>
          Close
        </button>
      </header>
      {error && (
        <p className="error" role="alert" data-testid="eval-results-error">
          {error}
        </p>
      )}
      {!results && !error && <p className="hint">Reading the results…</p>}
      {results && (
        <>
          <p className="hint" data-testid="eval-counts">
            {results.finished} of {results.n} finished
            {results.running > 0 ? ` · ${results.running} still going` : ""}
            {results.failed > 0 ? ` · ${results.failed} failed` : ""}. The figures cover the finished runs only.
          </p>
          <ul className="eval-strips" aria-label="Distributions">
            {MEASURES.map((m) => (
              <Strip key={m.key} measure={m} dist={results.distributions[m.key]} runs={results.runs} />
            ))}
          </ul>
          <table className="eval-table" aria-label="Eval runs">
            <thead>
              <tr>
                <th scope="col">Run</th>
                <th scope="col">Completed</th>
                <th scope="col">Checks</th>
                <th scope="col">Retries</th>
                <th scope="col">Tool failures</th>
                <th scope="col">Tokens</th>
                <th scope="col">Active</th>
              </tr>
            </thead>
            <tbody>
              {results.runs.map((run) => {
                const ended = run.status === "finished" || run.status === "failed";
                const cell = (value: string | number | null) => (ended && value !== null ? value : "—");
                return (
                  <tr key={run.index} data-testid={`eval-run-${run.index}`}>
                    <th scope="row">
                      {run.runDir && onOpenTrace ? (
                        <button type="button" className="link-button" onClick={() => onOpenTrace(run.runDir!)}>
                          {run.label}
                        </button>
                      ) : (
                        run.label
                      )}{" "}
                      <Pill tone={run.status === "finished" ? "success" : run.status === "failed" ? "danger" : "running"}>
                        {statusText(run)}
                      </Pill>
                    </th>
                    <td>{run.completed === null ? "—" : run.completed ? "yes" : "no"}</td>
                    <td data-testid={`eval-checks-${run.index}`}>
                      {run.status === "finished" ? evalCheckText(run.checkState, run.checkPassRate) : "—"}
                    </td>
                    <td>{cell(run.retries)}</td>
                    <td>{cell(run.toolFailures)}</td>
                    <td>{cell(run.tokens === null ? null : run.tokens.toLocaleString("en-US"))}</td>
                    <td>{cell(run.activeMinutes === null ? null : minutesText(run.activeMinutes))}</td>
                  </tr>
                );
              })}
            </tbody>
          </table>

          <div className="button-row">
            <button
              type="button"
              className="secondary-button"
              disabled={busy || finishedRuns.length === 0}
              onClick={() => (unchecked.length > 0 ? setConfirmClean(true) : void cleanUp())}
            >
              {busy ? "Cleaning up…" : "Clean up"}
            </button>
            <span className="hint">
              Removes the clean worktrees of the finished runs. The branches stay until you delete them.
            </span>
          </div>
          {lines && (
            <ul className="hint" aria-label="Clean up result" data-testid="eval-clean-lines">
              {lines.flatMap((l) =>
                l.lines.map((text, i) => (
                  <li key={`${l.index}:${i}`}>
                    Run {l.index}: {text}
                  </li>
                )),
              )}
            </ul>
          )}

          <h3>Kept branches</h3>
          {kept.length === 0 ? (
            <p className="hint">No branch is kept.</p>
          ) : (
            <ul className="eval-kept" aria-label="Kept branches">
              {kept.map(({ run, branch }) => (
                <li key={`${run.index}:${branch.prId}`} data-testid={`kept-${run.index}-${branch.prId}`}>
                  <code>{branch.branch}</code>{" "}
                  <span className="hint">
                    {branch.unmergedCommits === null
                      ? "unmerged commits not counted"
                      : `${branch.unmergedCommits} unmerged ${branch.unmergedCommits === 1 ? "commit" : "commits"}`}
                  </span>{" "}
                  <button
                    type="button"
                    className="link-button"
                    onClick={() => run.runDir && setDeleting({ runDir: run.runDir, branch })}
                  >
                    Delete…
                  </button>
                </li>
              ))}
            </ul>
          )}
        </>
      )}

      {confirmClean && (
        <Dialog
          title="Clean up this group?"
          onClose={() => setConfirmClean(false)}
          footer={
            <>
              <button type="button" className="secondary-button" onClick={() => setConfirmClean(false)}>
                Cancel
              </button>
              <button type="button" className="primary-button" onClick={() => void cleanUp()}>
                Clean up anyway
              </button>
            </>
          }
        >
          <p>
            {unchecked.map((r) => r.label).join(", ")} {unchecked.length === 1 ? "is" : "are"} not checked yet. Once the
            worktrees are gone, {unchecked.length === 1 ? "its" : "their"} check pass rate will stay &quot;not
            checked&quot;.
          </p>
        </Dialog>
      )}
      {deleting && (
        <Dialog
          title="Delete this branch?"
          onClose={() => setDeleting(null)}
          footer={
            <>
              <button type="button" className="secondary-button" onClick={() => setDeleting(null)}>
                Cancel
              </button>
              <button type="button" className="danger-button" onClick={() => void deleteBranch()}>
                Delete branch
              </button>
            </>
          }
        >
          <p>
            Delete <code>{deleting.branch.branch}</code>
            {deleting.branch.unmergedCommits === null
              ? ""
              : ` and its ${deleting.branch.unmergedCommits} unmerged ${deleting.branch.unmergedCommits === 1 ? "commit" : "commits"}`}
            ? Commits that exist on no other branch are lost. This cannot be undone.
          </p>
        </Dialog>
      )}
    </section>
  );
}
