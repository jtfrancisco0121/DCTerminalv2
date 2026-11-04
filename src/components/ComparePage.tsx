import { Fragment, useEffect, useMemo, useRef, useState, type KeyboardEvent } from "react";
import { flightCompare } from "../flight/api";
import type { CompareOut, CompareRow, CompareSide } from "../flight/types";
import { runOverview, type RunListItem } from "../run/api";
import {
  compareGroupText,
  compareKeyText,
  compareOpensTrace,
  compareSentence,
  compareValueText,
  outcomeText,
  outcomeTone,
} from "../run/words";
import { errorText } from "./InlineNotice";
import { Meter } from "./Meter";
import { Pill } from "./Pill";

type Props = {
  /** The run folders to compare when the page opens (either may be missing: the pickers fill them). */
  initialA?: string | null;
  initialB?: string | null;
  /** The pickers changed (the App keeps them while a Trace opened from here is shown). */
  onChange?: (a: string | null, b: string | null) => void;
  /** Open this run's Trace on this Event; Escape there returns here. */
  onOpenTrace: (runDir: string, eventId: string) => void;
  onClose: () => void;
};

/** A finished run: done, failed, or an eval run that ended (its record says so). */
function finishedRun(run: RunListItem): boolean {
  const outcome = run.record?.outcome ?? run.stage;
  return outcome === "done" || outcome === "failed" || outcome === "eval-done";
}

/** Same-folder runs first, otherwise the order of the list (newest first). */
function options(runs: RunListItem[], other: string | null): RunListItem[] {
  const folder = runs.find((r) => r.runDir === other)?.folder ?? null;
  if (!folder) return runs;
  return [...runs.filter((r) => r.folder === folder), ...runs.filter((r) => r.folder !== folder)];
}

const label = (run: RunListItem) => `${run.title || run.runId} (${run.runId})`;

function SideNote({ side }: { side: CompareSide }) {
  return (
    <p className="hint" data-testid={`compare-side-${side.runId}`}>
      <strong>{side.title || side.runId}</strong> {side.runId}{" "}
      {side.outcome && (
        <Pill tone={outcomeTone(side.outcome, null)}>{outcomeText(side.outcome)}</Pill>
      )}
    </p>
  );
}

/**
 * Compare two finished runs: the rows of both side by side, the first difference marked, `n` / `p` between the
 * differing rows, and a click on a value opens that run's Trace on the first Event of the row. Everything that
 * counts as a difference is decided by the backend (`flight_compare`); this page only shows it.
 */
export function ComparePage({ initialA = null, initialB = null, onChange, onOpenTrace, onClose }: Props) {
  const [runs, setRuns] = useState<RunListItem[]>([]);
  const [a, setA] = useState<string | null>(initialA);
  const [b, setB] = useState<string | null>(initialB);
  const [out, setOut] = useState<CompareOut | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  // Which of the differing rows the keyboard is on (0 = the first, the Divergence).
  const [cursor, setCursor] = useState(0);
  const page = useRef<HTMLElement>(null);
  const asked = useRef(0);

  useEffect(() => page.current?.focus(), []);

  useEffect(() => {
    let live = true;
    runOverview().then(
      (overview) => live && setRuns(overview.runs.filter(finishedRun)),
      () => live && setRuns([]),
    );
    return () => {
      live = false;
    };
  }, []);

  const same = a !== null && a === b;
  useEffect(() => onChange?.(a, b), [a, b]); // eslint-disable-line react-hooks/exhaustive-deps

  useEffect(() => {
    if (!a || !b || a === b) {
      setOut(null);
      setError(null);
      return;
    }
    const serial = ++asked.current;
    setBusy(true);
    flightCompare(a, b).then(
      (answer) => {
        if (serial !== asked.current) return;
        setOut(answer);
        setError(null);
        setCursor(0);
        setBusy(false);
      },
      (err: unknown) => {
        if (serial !== asked.current) return;
        setOut(null);
        setError(errorText(err));
        setBusy(false);
      },
    );
  }, [a, b]);

  const rows: CompareRow[] = out?.rows ?? [];
  const differing = useMemo(() => rows.map((r, i) => (r.differs ? i : -1)).filter((i) => i >= 0), [rows]);
  const current = differing[cursor] ?? null;
  const divergence = out?.divergence ?? null;
  const grid = useRef<HTMLTableElement>(null);

  // Open scrolled to the Divergence, and follow n / p.
  useEffect(() => {
    if (current === null) return;
    document.getElementById(`compare-row-${current}`)?.scrollIntoView?.({ block: "center" });
  }, [current, out]);

  const move = (by: number) => setCursor((c) => Math.max(0, Math.min(differing.length - 1, c + by)));
  const onKey = (event: KeyboardEvent<HTMLElement>) => {
    const target = event.target as HTMLElement;
    if (target.tagName === "SELECT" || target.tagName === "INPUT" || event.metaKey || event.ctrlKey || event.altKey) return;
    if (event.key === "n") move(1);
    else if (event.key === "p") move(-1);
  };

  const notReady = out ? [out.a, out.b].filter((s) => !s.indexed && !s.off) : [];
  const off = out ? [out.a, out.b].filter((s) => s.off) : [];
  const select = (name: string, value: string | null, set: (v: string | null) => void, other: string | null) => (
    <label className="trace-search-field">
      <span>{name}</span>
      <select value={value ?? ""} onChange={(e) => set(e.target.value || null)}>
        <option value="">Pick a finished run</option>
        {options(runs, other).map((r) => (
          <option key={r.runDir} value={r.runDir}>
            {label(r)}
          </option>
        ))}
      </select>
    </label>
  );

  let lastGroup = "";
  return (
    <section className="page compare-page" role="region" aria-label="Compare runs" tabIndex={-1} ref={page} onKeyDown={onKey}>
      <header className="page-header">
        <h2>Compare runs</h2>
        <button type="button" className="secondary-button" onClick={onClose}>
          Close
        </button>
      </header>

      <div className="compare-bar">
        {select("Run A", a, setA, b)}
        {select("Run B", b, setB, a)}
        <button type="button" className="secondary-button" disabled={cursor <= 0 || differing.length === 0} onClick={() => move(-1)}>
          Previous difference
        </button>
        <button
          type="button"
          className="secondary-button"
          disabled={differing.length === 0 || cursor >= differing.length - 1}
          onClick={() => move(1)}
        >
          Next difference
        </button>
      </div>

      {same && (
        <p className="hint" data-testid="compare-same">
          Pick two different runs.
        </p>
      )}
      {!same && (!a || !b) && <p className="hint">Pick two finished runs to compare.</p>}
      {busy && (
        <p className="hint" aria-busy="true">
          Comparing…
        </p>
      )}
      {error && (
        <p className="error" role="alert" data-testid="compare-error">
          Could not compare: {error}
        </p>
      )}

      {out && (
        <div className="compare-sides">
          <SideNote side={out.a} />
          <SideNote side={out.b} />
        </div>
      )}
      {off.map((s) => (
        <p key={s.runId} className="hint warn" data-testid="compare-off">
          {s.runId}: AgentFlight is off for this account (Settings › Activity).
        </p>
      ))}
      {notReady.map((s) => (
        <p key={s.runId} className="hint" data-testid="compare-not-indexed">
          Run {s.runId} is not indexed yet (Indexing {out?.progress.current ?? 0} of {out?.progress.total ?? 0} runs).{" "}
          <Meter
            value={out && out.progress.total > 0 ? (out.progress.current / out.progress.total) * 100 : null}
            tone="running"
            label={`Indexing ${out?.progress.current ?? 0} of ${out?.progress.total ?? 0} runs`}
          />
        </p>
      ))}

      {out && rows.length > 0 && (
        <>
          {divergence !== null ? (
            <p className="compare-first" data-testid="compare-first">
              {compareSentence(rows[divergence])}
            </p>
          ) : (
            <p className="hint" data-testid="compare-equal">
              No differences in these rows.
            </p>
          )}
          <table className="compare-grid" aria-label="Compare grid" ref={grid}>
            <caption className="hint">
              Run A {out.a.runId} against run B {out.b.runId}. The marked row is the first difference.
            </caption>
            <thead>
              <tr>
                <th scope="col">Row</th>
                <th scope="col">Run A</th>
                <th scope="col">Run B</th>
              </tr>
            </thead>
            <tbody>
              {rows.map((row, i) => {
                const heading = row.group !== lastGroup;
                lastGroup = row.group;
                const a_ = compareValueText(row.group, row.a);
                const b_ = compareValueText(row.group, row.b);
                const cell = (text: string, dir: string, event: string | null | undefined) =>
                  compareOpensTrace(row.group) && event ? (
                    <button
                      type="button"
                      className="link-button"
                      aria-label={`Open the trace of ${dir === out.a.runDir ? out.a.runId : out.b.runId} at ${compareKeyText(row.group, row.key)}`}
                      onClick={() => onOpenTrace(dir, event)}
                    >
                      {text}
                    </button>
                  ) : (
                    text
                  );
                return (
                  <Fragment key={`${row.group}:${row.key}`}>
                    {heading && (
                      <tr className="compare-group">
                        <th scope="rowgroup" colSpan={3}>
                          {compareGroupText(row.group)}
                        </th>
                      </tr>
                    )}
                    <tr
                      id={`compare-row-${i}`}
                      className={`compare-row${row.differs ? " differs" : " same"}${current === i ? " current" : ""}`}
                      aria-current={divergence === i ? "true" : undefined}
                      data-current={current === i ? "true" : undefined}
                      aria-label={`${compareGroupText(row.group)}: ${compareKeyText(row.group, row.key)}, run A ${a_}, run B ${b_}${row.differs ? ", differs" : ""}`}
                    >
                      <th scope="row">{compareKeyText(row.group, row.key)}</th>
                      <td>{cell(a_, out.a.runDir, row.aEvent)}</td>
                      <td>{cell(b_, out.b.runDir, row.bEvent)}</td>
                    </tr>
                  </Fragment>
                );
              })}
            </tbody>
          </table>
        </>
      )}
    </section>
  );
}
