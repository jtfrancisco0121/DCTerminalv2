import { useEffect, useMemo, useState } from "react";
import { openUrl, revealItemInDir } from "@tauri-apps/plugin-opener";
import { readRunFile, resumeRun, runOverview, type RunListItem, type StartedRun } from "../run/api";
import { costText } from "../run/observe";
import { minutesText, relativeDate, runMatches, safeUrl, stageText } from "../run/words";
import { Dialog } from "./Dialog";
import { errorText } from "./InlineNotice";
import { TextDialog } from "./RunPanel";
import { QueueSection } from "./QueueSection";

type Props = {
  windowId: string;
  onClose: () => void;
  /** A run was resumed into a tab. */
  onStarted: (started: StartedRun) => void;
};

/** All the app's runs, newest first, with search by title, folder or ticket id and a stage filter. */
export function RunsView({ windowId, onClose, onStarted }: Props) {
  const [runs, setRuns] = useState<RunListItem[] | null>(null);
  const [query, setQuery] = useState("");
  const [stage, setStage] = useState("");
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
  const shown = (runs ?? []).filter((run) => (!stage || (run.stage ?? "") === stage) && runMatches(run, query));

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
      <ul className="runs-list">
        {shown.map((run) => {
          const record = run.record;
          return (
            <li key={run.runDir} className="runs-row" data-testid={`run-${run.runId}`}>
              <div className="runs-main">
                <span className="run-list-title">{run.title ?? run.runId}</span>
                {run.adopted && <span className="hint"> (from the dev app)</span>}
                <span className="hint">
                  {" "}
                  — {stageText(run.stage)}
                  {run.folder ? ` · ${run.folder}` : ""}
                </span>
              </div>
              <div className="runs-facts hint">
                <span title={run.createdAt ?? undefined}>{relativeDate(run.createdAt) || "—"}</span>
                <span data-testid={`duration-${run.runId}`}>{minutesText(record?.minutes)}</span>
                <span
                  data-testid={`cost-${run.runId}`}
                  title="An estimate at API list prices, not a bill"
                >
                  {record ? `${costText(record.estimateUsd)} estimate` : "—"}
                </span>
                {(record?.prUrls ?? []).map((url, at) =>
                  safeUrl(url) ? (
                    <button key={url} type="button" className="link-button" onClick={() => void openUrl(url)}>
                      PR {at + 1}
                    </button>
                  ) : null,
                )}
                {!record?.prUrls?.length && run.prCount ? <span>{run.prCount} PRs</span> : null}
              </div>
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
            </li>
          );
        })}
      </ul>
      {report && <TextDialog title={report.title} text={report.text} onClose={() => setReport(null)} />}
    </Dialog>
  );
}
