import { useCallback, useEffect, useRef, useState } from "react";
import {
  flightReplayCheck,
  flightReplayCheckInfo,
  flightReplayCheckLog,
  flightReplayCheckStop,
  flightReplayChecks,
} from "../flight/api";
import type { RerunInfo, RerunRecord } from "../flight/types";
import {
  checkTone,
  lastLines,
  localTime,
  REPLAY_LABEL,
  RERUN_DIRTY_NOTE,
  RERUN_NOT_APP,
  rerunActive,
  rerunStateText,
  rerunStateTone,
  rerunTitle,
} from "../run/words";
import { Dialog } from "./Dialog";
import { errorText } from "./InlineNotice";
import { Pill } from "./Pill";

const POLL_MS = 1500;
const LOG_LINES = 40;

type Props = {
  runDir: string;
  /** The PR whose app check this row is. */
  pr: string;
  /** The row is the app's own check (not the skill's or a stop-check's). */
  appCheck: boolean;
  /** Test hook: how often a running re-run is read again. */
  pollMs?: number;
};

function ResultBlock({
  label,
  labelTone,
  pill,
  pillTone,
  exitCode,
  finishedAt,
  log,
  testId,
  onFullLog,
}: {
  label: string;
  labelTone: "muted" | "running" | "success" | "danger";
  pill: string;
  pillTone: "muted" | "running" | "success" | "danger" | "warn" | "you";
  exitCode: number | null;
  finishedAt: string | null;
  log?: string | null;
  testId: string;
  onFullLog?: () => void;
}) {
  const when = finishedAt ? localTime(finishedAt) : null;
  return (
    <div className="rerun-result" data-testid={testId}>
      <p>
        <Pill tone={labelTone}>{label}</Pill> <Pill tone={pillTone}>{pill}</Pill>
        {exitCode !== null && <span className="hint"> exit {exitCode}</span>}
        {when && (
          <span className="hint" title={when.title}>
            {" "}
            · {when.short}
          </span>
        )}
      </p>
      {log != null && (
        <>
          <pre className="rerun-log">{lastLines(log, LOG_LINES)}</pre>
          {onFullLog && (
            <button type="button" className="link-button" onClick={onFullLog}>
              Open full log
            </button>
          )}
        </>
      )}
    </div>
  );
}

/**
 * Re-run check: on an app `check.end` row in Replay mode. The button opens a confirmation that names the command, the
 * setup, the folder and the commit; "Re-run" starts it (two clicks). The recorded result and the latest re-run sit side
 * by side, earlier re-runs are listed by number, and a running one can be stopped. Closing the Trace does not stop it.
 */
export function RerunCheck({ runDir, pr, appCheck, pollMs = POLL_MS }: Props) {
  const [info, setInfo] = useState<RerunInfo | null>(null);
  const [records, setRecords] = useState<RerunRecord[]>([]);
  const [confirming, setConfirming] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [log, setLog] = useState<{ n: number; text: string; full: boolean } | null>(null);
  const [announce, setAnnounce] = useState("");
  const lastState = useRef<string>("");

  const mine = records.filter((r) => r.pr === pr);
  const latest = mine[0] ?? null;
  const active = latest !== null && rerunActive(latest.state);

  const refresh = useCallback(async () => {
    try {
      const [i, rs] = await Promise.all([appCheck ? flightReplayCheckInfo(runDir, pr) : Promise.resolve(null), flightReplayChecks(runDir)]);
      setInfo(i);
      setRecords(rs);
      setError(null);
    } catch (err) {
      setError(errorText(err));
    }
  }, [runDir, pr, appCheck]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  // A running re-run is read again until it ends.
  useEffect(() => {
    if (!active && !info?.running) return;
    const id = window.setInterval(() => void refresh(), pollMs);
    return () => window.clearInterval(id);
  }, [active, info?.running, refresh, pollMs]);

  // The log of the latest re-run, while it runs and once it ends.
  useEffect(() => {
    if (!latest) {
      setLog(null);
      return;
    }
    let live = true;
    flightReplayCheckLog(runDir, pr, latest.n).then(
      (text) => live && setLog((now) => ({ n: latest.n, text, full: now?.n === latest.n && now.full })),
      () => live && setLog(null),
    );
    return () => {
      live = false;
    };
  }, [runDir, pr, latest?.n, latest?.state]); // eslint-disable-line react-hooks/exhaustive-deps

  // The progress line is polite; the result is announced once.
  useEffect(() => {
    if (!latest) return;
    const key = `${latest.n}:${latest.state}`;
    if (key === lastState.current) return;
    lastState.current = key;
    setAnnounce(rerunActive(latest.state) ? `Re-run: ${rerunStateText(latest.state)}` : `Re-run finished: ${rerunStateText(latest.state)}`);
  }, [latest]);

  const reason = !appCheck ? RERUN_NOT_APP : (info?.reason ?? null);
  const start = async () => {
    setBusy(true);
    setError(null);
    try {
      await flightReplayCheck(runDir, pr);
      setConfirming(false);
      await refresh();
    } catch (err) {
      setError(errorText(err));
    } finally {
      setBusy(false);
    }
  };

  const stop = async () => {
    try {
      await flightReplayCheckStop();
    } catch (err) {
      setError(errorText(err));
    }
    await refresh();
  };

  return (
    <div className="rerun-check" data-testid="rerun-check">
      <p>
        <button
          type="button"
          className="secondary-button"
          disabled={reason !== null || (info === null && appCheck)}
          onClick={() => setConfirming(true)}
        >
          Re-run check
        </button>{" "}
        {reason && (
          <span className="hint" data-testid="rerun-reason">
            {reason}
          </span>
        )}
      </p>
      {error && (
        <p className="hint warn" role="alert" data-testid="rerun-error">
          {error}
        </p>
      )}
      <p className="visually-hidden" role="status" aria-live="polite" data-testid="rerun-live">
        {announce}
      </p>
      {(latest || info?.recorded) && (
        <div className="rerun-results">
          {info?.recorded && (
            <ResultBlock
              testId="rerun-recorded"
              label={REPLAY_LABEL.recorded}
              labelTone="muted"
              pill={`${REPLAY_LABEL.recorded}: ${info.recorded.result}`}
              pillTone={checkTone(info.recorded.result)}
              exitCode={info.recorded.exitCode}
              finishedAt={info.recorded.finishedAt}
            />
          )}
          {latest && (
            <ResultBlock
              testId="rerun-latest"
              label={REPLAY_LABEL.rerun}
              labelTone={rerunActive(latest.state) ? "running" : "muted"}
              pill={`${REPLAY_LABEL.rerun}: ${rerunStateText(latest.state)}`}
              pillTone={rerunStateTone(latest.state)}
              exitCode={latest.exitCode}
              finishedAt={latest.finishedAt}
              log={log?.n === latest.n ? log.text : null}
              onFullLog={log?.n === latest.n && !log.full ? () => setLog({ ...log, full: true }) : undefined}
            />
          )}
        </div>
      )}
      {log?.full && latest && (
        <pre className="rerun-log rerun-log-full" data-testid="rerun-full-log">
          {log.text}
        </pre>
      )}
      {latest?.detail && !rerunActive(latest.state) && latest.state !== "pass" && (
        <p className="hint" data-testid="rerun-detail">
          {latest.detail}
        </p>
      )}
      {latest?.scratchLeft && (
        <p className="hint warn">The scratch worktree could not be removed; it is retried when the app starts.</p>
      )}
      {active && (
        <p>
          <button type="button" className="secondary-button" onClick={() => void stop()}>
            Stop
          </button>
        </p>
      )}
      {mine.length > 1 && (
        <ul className="rerun-earlier" aria-label="Earlier re-runs">
          {mine.slice(1).map((r) => (
            <li key={r.n}>
              Re-run {r.n}: <Pill tone={rerunStateTone(r.state)}>{rerunStateText(r.state)}</Pill>
              {r.exitCode !== null && <span className="hint"> exit {r.exitCode}</span>}
            </li>
          ))}
        </ul>
      )}
      {confirming && info && (
        <Dialog
          title={rerunTitle(pr, info.commit)}
          onClose={() => setConfirming(false)}
          footer={
            <>
              <button type="button" className="secondary-button" onClick={() => setConfirming(false)}>
                Cancel
              </button>
              <button type="button" className="primary-button" disabled={busy} onClick={() => void start()}>
                Re-run
              </button>
            </>
          }
        >
          <div className="rerun-confirm" data-testid="rerun-confirm">
            {info.setup && (
              <p>
                Setup: <code>{info.setup}</code>
              </p>
            )}
            <p>
              Check: <code>{info.command}</code>
            </p>
            {info.cwd !== "" && (
              <p>
                Folder: <code>{info.cwd}</code>
              </p>
            )}
            <p>
              Commit: <code>{info.commit?.slice(0, 7)}</code>
            </p>
            {info.clean === false && <p className="hint warn">{RERUN_DIRTY_NOTE}</p>}
            <p className="hint">
              It runs in a new, temporary worktree at this commit and leaves your worktrees alone. This is a{" "}
              {REPLAY_LABEL.fresh}, not the recorded run.
            </p>
            {error && (
              <p className="hint warn" role="alert">
                {error}
              </p>
            )}
          </div>
        </Dialog>
      )}
    </div>
  );
}
