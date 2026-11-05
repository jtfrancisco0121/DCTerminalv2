import type { ReplayFileOut } from "../flight/types";
import { REPLAY_FOOTNOTE, REPLAY_LABEL, replayEditText } from "../run/words";
import { Pill } from "./Pill";

export type FileState =
  | { id: string; state: "loading" }
  | { id: string; state: "ready"; out: ReplayFileOut }
  | { id: string; state: "error"; message: string };

/** The warn line when a file stops being reconstructible. */
export const STOPPED_TEXT = "Cannot reconstruct this file past this edit.";

/**
 * The Reconstructed file at the selected `file.change`: its path, the "recorded" label, a redaction chip and
 * "after edit k of n", the text with line numbers, and the reason when the file cannot be shown.
 */
export function FilePane({ file }: { file: FileState }) {
  if (file.state === "loading") {
    return (
      <div className="file-pane" aria-busy="true" data-testid="file-pane">
        <p className="hint">Rebuilding the file…</p>
      </div>
    );
  }
  if (file.state === "error") {
    return (
      <div className="file-pane" data-testid="file-pane">
        <p className="error">{file.message}</p>
      </div>
    );
  }
  const out = file.out;
  const lines = out.text === null ? [] : out.text.split("\n");
  if (lines.length > 1 && lines[lines.length - 1] === "") lines.pop();
  return (
    <div className="file-pane" data-testid="file-pane">
      <div className="file-pane-header">
        <code className="file-pane-path">{out.path ?? "file"}</code>{" "}
        <Pill tone="muted" className="pill-replay-label">
          {REPLAY_LABEL.recorded}
        </Pill>
        {out.redactions > 0 && (
          <>
            {" "}
            <Pill tone="warn" testId="file-pane-redactions">{`${out.redactions} redacted`}</Pill>
          </>
        )}
        {out.edit !== null && out.edits > 0 && (
          <>
            {" "}
            <span className="hint" data-testid="file-pane-edit">
              {replayEditText(out.edit, out.edits)}
            </span>
          </>
        )}
      </div>
      {out.status === "off" && <p className="hint">{out.reason}</p>}
      {out.status === "unavailable" && (
        <p className="hint" data-testid="file-pane-reason">
          {out.reason}
        </p>
      )}
      {out.status === "ok" && out.reason && (
        <p className="hint" data-testid="file-pane-reason">
          {out.reason}
        </p>
      )}
      {out.stopped && (
        <p className="hint warn" role="status" data-testid="file-pane-stopped">
          {STOPPED_TEXT} {out.stopped.reason}
        </p>
      )}
      {out.status === "ok" && out.text !== null && (
        <pre className="file-pane-text" data-testid="file-pane-text">
          {lines.map((line, i) => (
            <span key={i} className="file-pane-line">
              <span className="file-pane-no" aria-hidden="true">
                {i + 1}
              </span>
              {line}
              {"\n"}
            </span>
          ))}
        </pre>
      )}
      {out.truncated && <p className="hint">The text is cut at 256 KiB.</p>}
      <p className="hint file-pane-footnote">{REPLAY_FOOTNOTE}</p>
    </div>
  );
}
