import { useState } from "react";
import { errorText } from "../components/InlineNotice";
import { clickupFetch, type ClickUpStart, type IntakePreview } from "./api";
import { parseIntakeRefs, type IntakeRef } from "./intakeRefs";

type Props = {
  windowId: string;
  /** The tickets Start takes (null: none). */
  onChange: (start: ClickUpStart | null) => void;
  disabled?: boolean;
};

/**
 * Tickets from ClickUp: paste task or list links (or ids), Fetch, remove the
 * ones you don't want. Start then snapshots the rest into `tickets.json`
 * and names them (id + title) in request.md. Uses the window's account token.
 */
export function ClickUpIntake({ windowId, onChange, disabled }: Props) {
  const [text, setText] = useState("");
  const [fetched, setFetched] = useState<{ refs: IntakeRef[]; preview: IntakePreview } | null>(null);
  const [removed, setRemoved] = useState<Set<string>>(new Set());
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const parsed = parseIntakeRefs(text);

  const kept = (fetched?.preview.tickets ?? []).filter((t) => !removed.has(t.id));

  const report = (next: typeof fetched, gone: Set<string>) => {
    const ids = (next?.preview.tickets ?? []).filter((t) => !gone.has(t.id)).map((t) => t.id);
    onChange(next && ids.length > 0 ? { refs: next.refs, ticketIds: ids } : null);
  };

  const fetchNow = async () => {
    setError(null);
    if (parsed.refs.length === 0) {
      setError(parsed.problems[0]?.message ?? "Paste a ClickUp task or list link first.");
      return;
    }
    setBusy(true);
    try {
      const preview = await clickupFetch(windowId, parsed.refs);
      const next = { refs: parsed.refs, preview };
      setFetched(next);
      setRemoved(new Set());
      report(next, new Set());
    } catch (err) {
      setError(errorText(err));
    } finally {
      setBusy(false);
    }
  };

  const clear = () => {
    setFetched(null);
    setRemoved(new Set());
    setText("");
    report(null, new Set());
    setError(null);
  };

  return (
    <section className="clickup-intake" aria-label="Tickets from ClickUp">
      <label className="field-label">
        Tickets from ClickUp (optional)
        <span className="field-row">
          <textarea
            className="text-input"
            rows={2}
            value={text}
            placeholder="Paste ClickUp task or list links, or task ids"
            disabled={disabled}
            onChange={(event) => setText(event.target.value)}
          />
          <button
            type="button"
            className="secondary-button"
            disabled={disabled || busy || !text.trim()}
            onClick={() => void fetchNow()}
          >
            {busy ? "Fetching…" : "Fetch"}
          </button>
        </span>
      </label>
      {parsed.problems.map((p) => (
        <p key={`parse-${p.line}`} className="hint warn">
          {p.message}
        </p>
      ))}
      {error && (
        <p className="error" role="alert">
          {error}
        </p>
      )}
      {fetched && (
        <div className="clickup-tickets" data-testid="clickup-tickets">
          {fetched.preview.problems.map((p) => (
            <p key={`fetch-${p.line}`} className="hint warn">
              Line {p.line}: {p.message}
            </p>
          ))}
          {kept.length === 0 && <p className="hint">No tickets will be taken.</p>}
          {kept.length > 0 && (
            <ul>
              {kept.map((t) => (
                <li key={t.id} data-testid={`ticket-${t.id}`}>
                  <strong>{t.customId ?? t.id}</strong> {t.title}
                  <span className="hint">
                    {" "}
                    — {t.status || "no status"}
                    {t.list ? ` · ${t.list}` : ""}
                  </span>
                  <button
                    type="button"
                    className="link-button"
                    aria-label={`Remove ${t.customId ?? t.id}`}
                    onClick={() => {
                      const gone = new Set(removed).add(t.id);
                      setRemoved(gone);
                      report(fetched, gone);
                    }}
                  >
                    remove
                  </button>
                </li>
              ))}
            </ul>
          )}
          {fetched.preview.excluded.length > 0 && (
            <p className="hint">
              Left out: {fetched.preview.excluded.map((t) => `${t.customId ?? t.id} (${t.reason})`).join("; ")}
            </p>
          )}
          <button type="button" className="link-button" onClick={clear}>
            Clear tickets
          </button>
        </div>
      )}
    </section>
  );
}
