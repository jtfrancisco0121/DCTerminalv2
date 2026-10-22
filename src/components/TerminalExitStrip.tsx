import { lastLines } from "../terminal/text";

/** The last non-empty line of a terminal's text, for the exit strip. */
export function lastOutputLine(text: string): string {
  const lines = lastLines(text, 50).split("\n");
  for (let index = lines.length - 1; index >= 0; index -= 1) {
    const line = lines[index].trim();
    if (line) return line;
  }
  return "";
}

/** The last 50 lines, for the clipboard. */
export function lastFiftyLines(text: string): string {
  return lastLines(text, 50);
}

type Props = {
  exitCode: number | null;
  /** The last non-empty line the process printed. */
  lastLine: string;
  /** Opening the process failed. */
  failed: string | null;
  /** `undefined` while checking, `null` when no session was saved. */
  resumeId: string | null | undefined;
  copied: boolean;
  onResume: () => void;
  onFresh: () => void;
  onCopy: () => void;
};

/** Shown at the top of a Claude terminal whose process ended. */
export function TerminalExitStrip({
  exitCode,
  lastLine,
  failed,
  resumeId,
  copied,
  onResume,
  onFresh,
  onCopy,
}: Props) {
  return (
    <div className="terminal-exit-strip" role="alert" aria-label="Terminal ended">
      <div className="terminal-exit-strip-text">
        {failed && <p className="error">{failed}</p>}
        {exitCode != null && <p>Process exited ({exitCode}).</p>}
        {lastLine && (
          <p className="terminal-exit-strip-last" title={lastLine}>
            {lastLine}
          </p>
        )}
        {resumeId === null && (
          <p className="terminal-exit-strip-reason">
            No saved Claude session for this terminal yet, so it cannot be resumed. It exited
            before the first prompt was saved to the session log.
          </p>
        )}
      </div>
      <div className="terminal-exit-strip-actions">
        {resumeId !== null && (
          <button
            type="button"
            className="primary-button"
            disabled={resumeId === undefined}
            onClick={onResume}
          >
            Resume session
          </button>
        )}
        <button type="button" className="secondary-button" onClick={onFresh}>
          Start fresh
        </button>
        <span className="terminal-exit-strip-note">Start fresh loses this conversation.</span>
        <button type="button" className="secondary-button" onClick={onCopy}>
          {copied ? "Copied" : "Copy last 50 lines"}
        </button>
      </div>
    </div>
  );
}
