import { save } from "@tauri-apps/plugin-dialog";
import { useCallback, useEffect, useRef, useState } from "react";
import { flightExportPreview, flightExportSave, flightExportSend } from "../flight/api";
import type { CapturePolicy, ExportEncoding, ExportPolicy, ExportPreview, TraceTarget } from "../flight/types";
import { EXPORT_CHANGED, EXPORT_TOO_LARGE, exportLeavesText, exportSizeText } from "../run/words";
import { Dialog } from "./Dialog";
import { errorText } from "./InlineNotice";

type Props = {
  target: TraceTarget;
  /** The Trace's capture policy: `full` is offered only for `full`. */
  capturePolicy: CapturePolicy;
  /** Names the default file (`<trace>.otlp.json` or `.otlp.pb`). */
  trace: string;
  onClose: () => void;
};

type Outcome = { kind: "sent"; text: string } | { kind: "saved"; text: string } | { kind: "failed"; text: string } | null;

/**
 * "Export": the Trace as OTLP. It opens on policy `metadata` and encoding `protobuf`, builds the Preview first
 * and enables Send and Save only for a built Preview; both act on exactly the previewed bytes. `full` is listed
 * only for a `full` capture policy and needs a confirmation that starts unticked for every export.
 */
export function ExportDialog({ target, capturePolicy, trace, onClose }: Props) {
  const [policy, setPolicy] = useState<ExportPolicy>("metadata");
  const [encoding, setEncoding] = useState<ExportEncoding>("protobuf");
  const [confirmFull, setConfirmFull] = useState(false);
  const [endpoint, setEndpoint] = useState("");
  const [preview, setPreview] = useState<ExportPreview | null>(null);
  const [loading, setLoading] = useState(true);
  const [problem, setProblem] = useState<string | null>(null);
  const [outcome, setOutcome] = useState<Outcome>(null);
  const [busy, setBusy] = useState(false);
  const serial = useRef(0);
  const targetKey = JSON.stringify(target);

  const build = useCallback(
    async (at: string | undefined) => {
      const mine = ++serial.current;
      setLoading(true);
      setProblem(null);
      setOutcome(null);
      try {
        const p = await flightExportPreview(target, {
          policy,
          encoding,
          confirmFull: policy === "full" && confirmFull,
          ...(at ? { endpoint: at } : {}),
        });
        if (mine !== serial.current) return;
        setPreview(p);
        setEndpoint((current) => (at === undefined || current.trim() === "" ? p.endpoint : at));
      } catch (err) {
        if (mine !== serial.current) return;
        setPreview(null);
        setProblem(errorText(err));
      } finally {
        if (mine === serial.current) setLoading(false);
      }
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [targetKey, policy, encoding, confirmFull],
  );

  // The first Preview uses Settings' endpoint (the host fills it in); a changed policy, encoding or tick
  // rebuilds it with the endpoint as typed.
  useEffect(() => {
    void build(endpoint.trim() === "" ? undefined : endpoint.trim());
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [build]);

  const tooLarge = problem === EXPORT_TOO_LARGE;
  const endpointMoved = preview !== null && endpoint.trim() !== preview.endpoint;
  const ready = preview !== null && !loading && !endpointMoved && !busy && outcome?.kind !== "sent";

  const send = async () => {
    if (!preview || !ready) return;
    setBusy(true);
    try {
      const sent = await flightExportSend(preview.digest, preview.endpoint);
      setOutcome({ kind: "sent", text: `Sent ${sent.spans} spans to ${sent.endpoint}.` });
    } catch (err) {
      setOutcome({ kind: "failed", text: errorText(err) });
    } finally {
      setBusy(false);
    }
  };

  const saveToFile = async () => {
    if (!preview || !ready) return;
    setBusy(true);
    try {
      const ext = preview.encoding === "json" ? "otlp.json" : "otlp.pb";
      const path = await save({ defaultPath: `${trace || "trace"}.${ext}` });
      if (path) {
        await flightExportSave(preview.digest, path);
        setOutcome({ kind: "saved", text: `Saved to ${path}.` });
      }
    } catch (err) {
      setOutcome({ kind: "failed", text: errorText(err) });
    } finally {
      setBusy(false);
    }
  };

  const changed = outcome?.kind === "failed" && outcome.text === EXPORT_CHANGED;

  return (
    <Dialog
      title="Export trace"
      onClose={onClose}
      className="export-dialog"
      footer={
        <>
          <button type="button" className="secondary-button" onClick={onClose}>
            Close
          </button>
          <button type="button" className="secondary-button" disabled={!ready} onClick={() => void saveToFile()}>
            Save to file
          </button>
          <button type="button" className="primary-button" disabled={!ready} onClick={() => void send()}>
            Send
          </button>
        </>
      }
    >
      <label className="field-label">
        Policy
        <select
          className="text-input"
          aria-label="Export policy"
          value={policy}
          onChange={(event) => {
            setPolicy(event.target.value as ExportPolicy);
            setConfirmFull(false);
          }}
        >
          <option value="metadata">metadata (no tool text)</option>
          {capturePolicy === "full" && <option value="full">full (redacted tool text)</option>}
        </select>
      </label>
      {policy === "full" && (
        <label className="check-label">
          <input type="checkbox" checked={confirmFull} onChange={(event) => setConfirmFull(event.target.checked)} /> I
          confirm that the redacted tool arguments and results leave this machine in this export.
        </label>
      )}
      <label className="field-label">
        Encoding
        <select
          className="text-input"
          aria-label="Export encoding"
          value={encoding}
          onChange={(event) => setEncoding(event.target.value as ExportEncoding)}
        >
          <option value="protobuf">protobuf (application/x-protobuf)</option>
          <option value="json">JSON (application/json)</option>
        </select>
      </label>
      <label className="field-label">
        Endpoint
        <input
          className="text-input"
          aria-label="Export endpoint"
          value={endpoint}
          onChange={(event) => setEndpoint(event.target.value)}
          onKeyDown={(event) => {
            // Enter rebuilds the Preview; it never sends.
            if (event.key === "Enter") {
              event.preventDefault();
              void build(endpoint.trim() || undefined);
            }
          }}
        />
      </label>
      {endpointMoved && !loading && (
        <p className="hint" data-testid="export-endpoint-moved">
          Press Enter to preview for this endpoint.
        </p>
      )}

      {loading && (
        <p className="hint" role="status" aria-busy="true" data-testid="export-loading">
          Building the preview…
        </p>
      )}
      {tooLarge && (
        <p className="hint warn" role="alert" data-testid="export-too-large">
          {problem}
        </p>
      )}
      {problem && !tooLarge && (
        <p className="hint warn" role="alert" data-testid="export-problem">
          {problem}
        </p>
      )}
      {preview && !loading && (
        <div data-testid="export-preview">
          <p data-testid="export-size">{exportSizeText(preview.spans, preview.events, preview.bytes)}</p>
          {preview.warnings.map((w) => (
            <p key={w} className="hint warn">
              {w}
            </p>
          ))}
          {!preview.loopback && (
            <p className="hint warn" role="alert" data-testid="export-leaves">
              {exportLeavesText(preview.spans, preview.endpoint)}
            </p>
          )}
          <details>
            <summary>Fields that leave ({preview.fields.length})</summary>
            <table className="export-fields">
              <thead>
                <tr>
                  <th>Field</th>
                  <th>Carried by</th>
                </tr>
              </thead>
              <tbody>
                {preview.fields.map((f) => (
                  <tr key={f.field}>
                    <td>{f.field}</td>
                    <td>{f.count}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </details>
        </div>
      )}

      {outcome && outcome.kind !== "failed" && (
        <p role="status" data-testid="export-outcome">
          {outcome.text}
        </p>
      )}
      {outcome?.kind === "failed" && (
        <p className="hint warn" role="alert" data-testid="export-failed">
          {changed ? "The trace changed since the preview; preview again." : outcome.text}{" "}
          {changed && (
            <button type="button" className="link-button" onClick={() => void build(endpoint.trim() || undefined)}>
              Preview again
            </button>
          )}
        </p>
      )}
    </Dialog>
  );
}
