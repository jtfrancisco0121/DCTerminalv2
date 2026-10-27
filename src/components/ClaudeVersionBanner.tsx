import { useEffect, useState } from "react";
import {
  claudeVersionStatus,
  runClaudeSelftest,
  selftestSteps,
  versionNote,
  versionWarning,
  type ClaudeVersionStatus,
} from "../run/observe";
import { reportError } from "../uiErrors";
import { errorText } from "./InlineNotice";

type ViewProps = {
  status: ClaudeVersionStatus;
  running: boolean;
  problem: string | null;
  onSelftest: () => void;
  /** A self-test ran in this window: list its steps even when it passed. */
  showResult?: boolean;
};

/** The self-test's steps, each pass or fail, and how long it took. */
export function SelftestSteps({ test }: { test: NonNullable<ClaudeVersionStatus["selftest"]> }) {
  return (
    <div data-testid="selftest-steps">
      <ul className="selftest-steps">
        {selftestSteps(test).map((step) => (
          <li key={step.label} className={step.ok ? "selftest-pass" : "selftest-fail"}>
            {step.ok ? "✓" : "✗"} {step.label}
          </li>
        ))}
      </ul>
      <p className="hint">took {Math.round(test.seconds)} s</p>
    </div>
  );
}

/** R3: out-of-range warning, and the one-click self-test after a version change. */
export function ClaudeVersionBannerView({ status, running, problem, onSelftest, showResult = false }: ViewProps) {
  const warning = versionWarning(status);
  const note = versionNote(status);
  const last = status.selftest;
  const failed = last && !last.ok && last.version && status.version?.startsWith(last.version);
  const listed = Boolean(last && (failed || showResult));
  if (!warning && !note && !status.selftestWanted && !failed && !running && !problem && !listed) return null;
  return (
    <div className="version-banner" role="status" data-testid="version-banner">
      {warning && <p className="hint warn">{warning}</p>}
      {note && <p className="hint">{note}</p>}
      {status.selftestWanted && !running && (
        <p className="hint">
          Claude Code {status.version} has not been self-tested with the dcv2 plugin.{" "}
          <button type="button" className="link-button" onClick={onSelftest}>
            Run the 30 s self-test (haiku)
          </button>
        </p>
      )}
      {running && <p className="hint">Self-test running…</p>}
      {failed && !running && (
        <p className="hint warn">
          Self-test failed: {last.detail ?? "unknown reason"}{" "}
          <button type="button" className="link-button" onClick={onSelftest}>
            Run again
          </button>
        </p>
      )}
      {last && listed && !running && <SelftestSteps test={last} />}
      {problem && <p className="hint warn">{problem}</p>}
    </div>
  );
}

export function ClaudeVersionBanner({ windowId }: { windowId: string }) {
  const [status, setStatus] = useState<ClaudeVersionStatus | null>(null);
  const [running, setRunning] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const [ran, setRan] = useState(false);

  useEffect(() => {
    claudeVersionStatus(windowId).then(setStatus, (err: unknown) => {
      reportError("Claude version check", err, { once: "claude-version" });
      setStatus(null);
    });
  }, [windowId]);

  const selftest = () => {
    setRunning(true);
    setProblem(null);
    runClaudeSelftest(windowId)
      .then(
        () =>
          claudeVersionStatus(windowId).then((next) => {
            setStatus(next);
            setRan(true);
          }),
        (err: unknown) => setProblem(errorText(err)),
      )
      .finally(() => setRunning(false));
  };

  if (!status) return null;
  return <ClaudeVersionBannerView status={status} running={running} problem={problem} onSelftest={selftest} showResult={ran} />;
}
