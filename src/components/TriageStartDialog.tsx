import { useEffect, useState } from "react";
import { recentFolders } from "../bridge";
import { triageBlocking, triageStart } from "../clickup/api";
import { parseIntakeRefs } from "../clickup/intakeRefs";
import { Dialog } from "./Dialog";
import { errorText } from "./InlineNotice";

/** The ClickUp list a pasted link or id names, or null. */
export function listIdOf(text: string): string | null {
  const trimmed = text.trim();
  if (/^\d{4,64}$/.test(trimmed)) return trimmed;
  const parsed = parseIntakeRefs(trimmed);
  return parsed.problems.length === 0 ? (parsed.refs.find((r) => r.kind === "list")?.id ?? null) : null;
}

/**
 * "Triage open tickets…": pick the ClickUp list (paste its link or id) and the folder the run starts in. Start is
 * disabled, with the reason, when no list is named, no folder is chosen, or the list's newest triage run is unfinished.
 */
export function TriageStartDialog({ windowId, onClose, onStarted }: { windowId: string; onClose: () => void; onStarted: () => void }) {
  const [list, setList] = useState("");
  const [folder, setFolder] = useState("");
  const [folders, setFolders] = useState<string[]>([]);
  const [blocking, setBlocking] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const listId = listIdOf(list);

  useEffect(() => {
    void recentFolders().then(
      (f) => {
        setFolders(f);
        setFolder((now) => now || f[0] || "");
      },
      () => {},
    );
  }, []);
  useEffect(() => {
    setBlocking(null);
    if (!listId) return;
    let live = true;
    void triageBlocking(listId).then(
      (why) => live && setBlocking(why),
      () => {},
    );
    return () => {
      live = false;
    };
  }, [listId]);

  const reason = !listId
    ? "Paste a ClickUp list link or its id."
    : !folder.trim()
      ? "Choose the folder the run starts in."
      : blocking
        ? `Cannot start: ${blocking}.`
        : null;

  const start = async () => {
    if (!listId) return;
    setBusy(true);
    setError(null);
    try {
      await triageStart(windowId, listId, folder.trim());
      onStarted();
      onClose();
    } catch (err) {
      setError(errorText(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog
      title="Triage open tickets"
      onClose={onClose}
      footer={
        <div className="button-row">
          <button type="button" className="primary-button" disabled={busy || reason !== null} title={reason ?? undefined} onClick={() => void start()}>
            Start triage
          </button>
          <button type="button" className="secondary-button" onClick={onClose}>
            Cancel
          </button>
        </div>
      }
    >
      <p className="hint">
        Reads the open tickets of a ClickUp list (at most 100, newest first) and proposes the next runs. Nothing is sent to ClickUp until you approve.
      </p>
      <label className="field-label">
        ClickUp list (link or id)
        <input className="text-input" aria-label="ClickUp list" value={list} placeholder="https://app.clickup.com/…/v/li/901800112233" onChange={(event) => setList(event.target.value)} />
      </label>
      <label className="field-label">
        Folder the run starts in
        <input className="text-input" aria-label="Triage folder" list="triage-folders" value={folder} onChange={(event) => setFolder(event.target.value)} />
        <datalist id="triage-folders">
          {folders.map((f) => (
            <option key={f} value={f} />
          ))}
        </datalist>
      </label>
      {reason && (
        <p className="hint" role="status" data-testid="triage-start-reason">
          {reason}
        </p>
      )}
      {error && (
        <p className="error" role="alert">
          {error}
        </p>
      )}
    </Dialog>
  );
}
