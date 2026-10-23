import { useEffect, useState } from "react";
import { claudeVersionStatus, runClaudeSelftest, versionWarning, type ClaudeVersionStatus } from "../run/observe";
import { errorText } from "./InlineNotice";

type ViewProps = {
  status: ClaudeVersionStatus;
  running: boolean;
  problem: string | null;
  onSelftest: () => void;
};

/** R3: out-of-range warning, and the one-click self-test after a version change. */
export function ClaudeVersionBannerView({ status, running, problem, onSelftest }: ViewProps) {
  const warning = versionWarning(status);
  const last = status.selftest;
  const failed = last && !last.ok && last.version && status.version?.startsWith(last.version);
  if (!warning && !status.selftestWanted && !failed && !running && !problem) return null;
  return (
    <div className="version-banner" role="status" data-testid="version-banner">
      {warning && <p className="hint warn">{warning}</p>}
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
      {problem && <p className="hint warn">{problem}</p>}
    </div>
  );
}

export function ClaudeVersionBanner({ windowId }: { windowId: string }) {
  const [status, setStatus] = useState<ClaudeVersionStatus | null>(null);
  const [running, setRunning] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);

  useEffect(() => {
    claudeVersionStatus(windowId).then(setStatus, () => setStatus(null));
  }, [windowId]);

  const selftest = () => {
    setRunning(true);
    setProblem(null);
    runClaudeSelftest(windowId)
      .then(
        () => claudeVersionStatus(windowId).then(setStatus),
        (err: unknown) => setProblem(errorText(err)),
      )
      .finally(() => setRunning(false));
  };

  if (!status) return null;
  return <ClaudeVersionBannerView status={status} running={running} problem={problem} onSelftest={selftest} />;
}
