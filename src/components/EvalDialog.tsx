import { useEffect, useState } from "react";
import { flightRunShare } from "../flight/api";
import { dispatchEvalPreview, dispatchEvalStart, type EvalPreview } from "../run/dispatch";
import { evalBaseText, evalEstimateText, evalHoldReason, evalUsageLine } from "../run/words";
import { Dialog } from "./Dialog";
import { errorText } from "./InlineNotice";

export const MIN_RUNS = 2;
export const MAX_RUNS = 10;
export const DEFAULT_RUNS = 3;

type Props = {
  windowId: string | null;
  /** The finished run to repeat. */
  runDir: string;
  onClose: () => void;
  /** The group was queued: `n` runs under this batch id. */
  onQueued: (n: number, group: string) => void;
  /** Seconds since the epoch; a test passes its own. */
  nowSec?: () => number;
};

/**
 * "Evaluate N times…": the same request, N runs on the dispatch queue. It shows N, where the runs build, the current
 * five-hour usage against the hold, and an estimate of the usage N runs cost (N times the source's Window share).
 * Start is disabled with the reason while the usage hold applies or the usage is not known; the backend checks the
 * same rule again and its refusal is shown here.
 */
export function EvalDialog({ windowId, runDir, onClose, onQueued, nowSec = () => Math.floor(Date.now() / 1000) }: Props) {
  const [n, setN] = useState(DEFAULT_RUNS);
  const [preview, setPreview] = useState<EvalPreview | null>(null);
  const [share, setShare] = useState<number | null | undefined>(undefined);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    let live = true;
    dispatchEvalPreview(windowId, runDir).then(
      (p) => live && setPreview(p),
      (err: unknown) => live && setError(errorText(err)),
    );
    flightRunShare(runDir).then(
      (s) => live && setShare(s),
      () => live && setShare(null),
    );
    return () => {
      live = false;
    };
  }, [windowId, runDir]);

  const valid = Number.isInteger(n) && n >= MIN_RUNS && n <= MAX_RUNS;
  const reason = preview
    ? evalHoldReason(preview.holdPercent, preview.fiveHour, nowSec())
    : null;
  const blocked = !preview || !valid || reason !== null;

  const start = async () => {
    if (blocked || busy) return;
    setBusy(true);
    try {
      const group = await dispatchEvalStart(windowId, runDir, n);
      onQueued(n, group);
    } catch (err) {
      setError(errorText(err));
      setBusy(false);
    }
  };

  return (
    <Dialog
      title="Evaluate this run"
      onClose={onClose}
      className="eval-dialog"
      footer={
        <>
          <button type="button" className="secondary-button" onClick={onClose}>
            Cancel
          </button>
          <button type="button" className="primary-button" disabled={blocked || busy} onClick={() => void start()}>
            {busy ? "Queueing…" : valid ? `Start ${n} runs` : "Start"}
          </button>
        </>
      }
    >
      <p className="hint" data-testid="eval-title">
        {preview?.title ? <strong>{preview.title}</strong> : "Reading the run…"}
      </p>
      <label className="field-label">
        Runs
        <input
          type="number"
          className="text-input text-input-narrow"
          aria-label="Number of runs"
          min={MIN_RUNS}
          max={MAX_RUNS}
          value={Number.isNaN(n) ? "" : n}
          onChange={(event) => setN(event.target.value === "" ? Number.NaN : Number(event.target.value))}
        />
      </label>
      {!valid && (
        <p className="hint warn" data-testid="eval-range">
          Pick between {MIN_RUNS} and {MAX_RUNS} runs.
        </p>
      )}
      <p className="hint" data-testid="eval-usage">
        {preview ? evalUsageLine(preview.holdPercent, preview.fiveHour, nowSec()) : "Reading the usage…"}
      </p>
      <p className="hint" data-testid="eval-base">
        {preview ? evalBaseText(preview.basedRepos, preview.defaultBranchRepos) : ""}
      </p>
      <p className="hint" data-testid="eval-estimate">
        {share === undefined ? "Estimate: reading…" : evalEstimateText(valid ? n : DEFAULT_RUNS, share)}
      </p>
      <p className="hint">Nothing is pushed or posted. The runs wait in the queue and respect the usage hold.</p>
      {reason && (
        <p className="hint warn" role="alert" data-testid="eval-reason">
          {reason}
        </p>
      )}
      {error && (
        <p className="error" role="alert" data-testid="eval-error">
          {error}
        </p>
      )}
    </Dialog>
  );
}
