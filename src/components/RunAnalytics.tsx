import { useEffect, useMemo, useRef, useState } from "react";
import { runOverview, type RunListItem } from "../run/api";
import { filterOptions, trendSeries } from "../run/analytics";
import { costText } from "../run/observe";
import { ESTIMATE_NOTE, ESTIMATES_CAPTION, minutesText } from "../run/words";
import { BarChart, LineChart } from "./Chart";
import { errorText } from "./InlineNotice";

type Props = { onClose: () => void };

const percent = (value: number) => `${Math.round(value * 100)}%`;
const notShown = (count: number, why: string) => (count > 0 ? <p className="hint">{count} not shown: {why}.</p> : null);

/** Trends over the finished runs: estimate, duration and wait share. Read once when the page opens. */
export function RunAnalytics({ onClose }: Props) {
  const [runs, setRuns] = useState<RunListItem[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [folder, setFolder] = useState("");
  const [repo, setRepo] = useState("");

  useEffect(() => {
    runOverview().then(
      (overview) => setRuns(overview.runs),
      (err: unknown) => setError(errorText(err)),
    );
  }, []);

  // The page takes the keyboard from a terminal underneath (Escape must not reach it).
  const page = useRef<HTMLElement>(null);
  useEffect(() => page.current?.focus(), []);

  const options = useMemo(() => filterOptions(runs ?? []), [runs]);
  const trend = useMemo(() => trendSeries(runs ?? [], { folder, repo }), [runs, folder, repo]);

  return (
    <section className="page" role="region" aria-label="Run analytics" tabIndex={-1} ref={page}>
      <header className="page-header">
        <h2>Run analytics</h2>
        <button type="button" className="secondary-button" onClick={onClose}>
          Close
        </button>
      </header>
      {error && (
        <p className="error" role="alert">
          {error}
        </p>
      )}
      {runs === null && !error && <p className="hint">Reading the runs…</p>}
      {runs && (
        <>
          <div className="runs-filters">
            <select className="text-input text-input-narrow" aria-label="Folder" value={folder} onChange={(e) => setFolder(e.target.value)}>
              <option value="">All folders</option>
              {options.folders.map((value) => (
                <option key={value} value={value}>
                  {value}
                </option>
              ))}
            </select>
            <select className="text-input text-input-narrow" aria-label="Repo" value={repo} onChange={(e) => setRepo(e.target.value)}>
              <option value="">All repos</option>
              {options.repos.map((value) => (
                <option key={value} value={value}>
                  {value}
                </option>
              ))}
            </select>
          </div>
          <section className="chart-block" aria-label="Estimated cost per run">
            <h3>Estimated cost per run</h3>
            <p className="hint" title={ESTIMATE_NOTE}>
              {ESTIMATES_CAPTION}
            </p>
            <BarChart title="Estimated cost per run" points={trend.estimate} format={costText} />
            {notShown(trend.skipped.estimate, "no price for the run's model")}
          </section>
          <section className="chart-block" aria-label="Duration per run">
            <h3>Duration per run</h3>
            <BarChart title="Duration per run" points={trend.minutes} format={minutesText} />
            {notShown(trend.skipped.minutes, "no duration")}
          </section>
          <section className="chart-block" aria-label="Wait share per run">
            <h3>Share of the run spent waiting for you</h3>
            <LineChart title="Wait share per run" points={trend.waitShare} format={percent} />
            {notShown(trend.skipped.waitShare, "no wait figure (an old record without a log, or a run with no span)")}
          </section>
        </>
      )}
    </section>
  );
}
