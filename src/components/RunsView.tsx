import { useEffect, useMemo, useState } from "react";
import { openUrl, revealItemInDir } from "@tauri-apps/plugin-opener";
import { readRunFile, resumeRun, runOverview, type RunListItem, type StartedRun } from "../run/api";
import { costText } from "../run/observe";
import { runsSummary, sortRuns, type SortDir, type SortKey } from "../run/analytics";
import { ESTIMATE_NOTE, minutesText, outcomeTone, relativeDate, runMatches, safeUrl, stageText } from "../run/words";
import { Pill } from "./Pill";
import { Dialog } from "./Dialog";
import { errorText } from "./InlineNotice";
import { TextDialog } from "./RunPanel";
import { QueueSection } from "./QueueSection";
import { BatchOverview } from "./BatchOverview";
import { EpicsSection } from "./EpicsSection";

type Props = {
  windowId: string;
  onClose: () => void;
  /** A run was resumed into a tab. */
  onStarted: (started: StartedRun) => void;
  /** The Analytics button: open the analytics page (the owner closes this dialog). */
  onAnalytics?: () => void;
};

/** All the app's runs, newest first, with search by title, folder or ticket id and a stage filter. */
export function RunsView({ windowId, onClose, onStarted, onAnalytics }: Props) {
  const [runs, setRuns] = useState<RunListItem[] | null>(null);
  const [query, setQuery] = useState("");
  const [stage, setStage] = useState("");
  // Duration and Estimate sort: click for ascending, again for descending, again for newest first.
  const [sort, setSort] = useState<{ key: SortKey; dir: SortDir } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [report, setReport] = useState<{ title: string; text: string } | null>(null);

  useEffect(() => {
    runOverview().then(
      (overview) => setRuns(overview.runs),
      (err: unknown) => setError(errorText(err)),
    );
  }, []);

  const stages = useMemo(
    () => Array.from(new Set((runs ?? []).map((run) => run.stage ?? "")).values()).sort(),
    [runs],
  );
  const filtered = (runs ?? []).filter((run) => (!stage || (run.stage ?? "") === stage) && runMatches(run, query));
  const shown = sortRuns(filtered, sort?.key ?? null, sort?.dir);
  const summary = runsSummary(filtered, new Date());

  const toggleSort = (key: SortKey) =>
    setSort((current) => {
      if (current?.key !== key) return { key, dir: "asc" };
      return current.dir === "asc" ? { key, dir: "desc" } : null;
    });
  const ariaSort = (key: SortKey) =>
    sort?.key === key ? (sort.dir === "asc" ? "ascending" : "descending") : "none";
  const sortHeader = (key: SortKey, label: string) => (
    <th scope="col" aria-sort={ariaSort(key)}>
      <button type="button" className="link-button" onClick={() => toggleSort(key)}>
        {label}
        {sort?.key === key ? (sort.dir === "asc" ? " ▲" : " ▼") : ""}
      </button>
    </th>
  );

  const showReport = (run: RunListItem) =>
    readRunFile(run.runDir, "report.md").then(
      (text) => setReport({ title: `Report: ${run.title ?? run.runId}`, text }),
      (err: unknown) => setError(errorText(err)),
    );

  const resume = async (run: RunListItem) => {
    setBusy(true);
    setError(null);
    try {
      onStarted(await resumeRun(windowId, run.runDir));
    } catch (err) {
      setError(errorText(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog title="Runs" onClose={onClose} className="runs-dialog">
      <QueueSection />
      <BatchOverview />
      <EpicsSection />
      <div className="runs-filters">
        <input
          className="text-input"
          type="search"
          aria-label="Search runs"
          placeholder="Search by title, folder or ticket id"
          value={query}
          onChange={(event) => setQuery(event.target.value)}
        />
        <select
          className="text-input text-input-narrow"
          aria-label="Stage"
          value={stage}
          onChange={(event) => setStage(event.target.value)}
        >
          <option value="">All stages</option>
          {stages.map((value) => (
            <option key={value} value={value}>
              {value ? stageText(value) : "No stage yet"}
            </option>
          ))}
        </select>
      </div>
      {error && (
        <p className="error" role="alert">
          {error}
        </p>
      )}
      {runs === null && !error && <p className="hint">Reading the runs…</p>}
      {runs && shown.length === 0 && <p className="hint">No runs match.</p>}
      {runs && (
        <p className="runs-summary" data-testid="runs-summary">
          <span>
            {summary.count} {summary.count === 1 ? "run" : "runs"} in the last {summary.days} days
          </span>
          <span title={ESTIMATE_NOTE}>
            Estimate {summary.estimateUsd == null ? "—" : costText(summary.estimateUsd)}
            {summary.unpriced > 0 ? ` (${summary.unpriced} unpriced)` : ""}
          </span>
          <span>Median duration {minutesText(summary.medianMinutes)}</span>
          {onAnalytics && (
            <button type="button" className="link-button" onClick={onAnalytics}>
              Analytics
            </button>
          )}
        </p>
      )}
      {shown.length > 0 && (
        <div className="runs-table-wrap">
          <table className="runs-table">
            <thead>
              <tr>
                <th scope="col">Run</th>
                <th scope="col">Outcome</th>
                <th scope="col">Started</th>
                {sortHeader("duration", "Duration")}
                {sortHeader("estimate", "Estimate")}
                <th scope="col">PRs</th>
                <th scope="col">Actions</th>
              </tr>
            </thead>
            <tbody>
              {shown.map((run) => {
                const record = run.record;
                return (
                  <tr key={run.runDir} className="runs-row" data-testid={`run-${run.runId}`}>
                    <td className="runs-main">
                      <span className="run-list-title">{run.title ?? run.runId}</span>
                      {run.adopted && <span className="hint"> (from the dev app)</span>}
                      <span className="hint">
                        {" "}
                        — {stageText(run.stage)}
                        {run.folder ? ` · ${run.folder}` : ""}
                      </span>
                      {(run.ticketIds ?? []).length > 0 && (
                        <span className="hint" data-testid={`tickets-${run.runId}`}>
                          {" "}
                          · {(run.ticketIds ?? []).join(", ")}
                        </span>
                      )}
                    </td>
                    <td>
                      <Pill tone={outcomeTone(record?.outcome, run.stage)}>{stageText(record?.outcome ?? run.stage)}</Pill>
                    </td>
                    <td className="hint" title={run.createdAt ?? undefined}>
                      {relativeDate(run.createdAt) || "—"}
                    </td>
                    <td className="hint" data-testid={`duration-${run.runId}`}>
                      {minutesText(record?.minutes)}
                    </td>
                    <td className="hint" data-testid={`cost-${run.runId}`} title={ESTIMATE_NOTE}>
                      {record ? `${costText(record.estimateUsd)} estimate` : "—"}
                    </td>
                    <td>
                      <div className="runs-facts hint">
                      {(record?.prUrls ?? []).map((url, at) =>
                        safeUrl(url) ? (
                          <button key={url} type="button" className="link-button" onClick={() => void openUrl(url)}>
                            PR {at + 1}
                          </button>
                        ) : null,
                      )}
                      {!record?.prUrls?.length && run.prCount ? <span>{run.prCount} PRs</span> : null}
                      </div>
                    </td>
                    <td>
                      <div className="runs-actions">
                      {run.hasReport && (
                        <button type="button" className="link-button" onClick={() => void showReport(run)}>
                          Report
                        </button>
                      )}
                      <button
                        type="button"
                        className="link-button"
                        onClick={() => void revealItemInDir(run.runDir).catch((err: unknown) => setError(errorText(err)))}
                      >
                        Open folder
                      </button>
                      {run.stage !== "done" && (
                        <button type="button" className="link-button" disabled={busy} onClick={() => void resume(run)}>
                          Resume
                        </button>
                      )}
                      </div>
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      )}
      {report && <TextDialog title={report.title} text={report.text} onClose={() => setReport(null)} />}
    </Dialog>
  );
}
