import { useEffect, useState } from "react";
import { getSettings, type WebQaTarget } from "../bridge";
import { webqaStart, type StartedRun } from "../run/api";
import { Dialog } from "./Dialog";
import { errorText } from "./InlineNotice";

/**
 * "Start web QA pass": pick the target, tick the charters. Start is disabled, with the reason, when the target has none.
 * `initialTarget` preselects a target (Rerun).
 */
export function WebQaStartDialog({
  windowId,
  initialTarget,
  onClose,
  onStarted,
}: {
  windowId: string;
  initialTarget?: string | null;
  onClose: () => void;
  onStarted: (run: StartedRun) => void;
}) {
  const [targets, setTargets] = useState<WebQaTarget[] | null>(null);
  const [targetId, setTargetId] = useState(initialTarget ?? "");
  const [ticked, setTicked] = useState<Set<string>>(new Set());
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    void getSettings().then(
      (view) => {
        const list = view.webQa?.targets ?? [];
        setTargets(list);
        setTargetId((now) => (list.some((t) => t.id === now) ? now : (list[0]?.id ?? "")));
      },
      (err: unknown) => setError(errorText(err)),
    );
  }, []);

  const target = targets?.find((t) => t.id === targetId) ?? null;
  // Every charter starts ticked: a pass tries them all unless the operator unticks some.
  useEffect(() => setTicked(new Set(target?.charters.map((c) => c.id) ?? [])), [target?.id]);

  const reason = !target
    ? "Add a target in Settings › Web QA first."
    : target.charters.length === 0
      ? "Add a charter in Settings › Web QA"
      : ticked.size === 0
        ? "Tick at least one charter."
        : null;

  const start = async () => {
    if (!target) return;
    setBusy(true);
    setError(null);
    try {
      const ids = target.charters.filter((c) => ticked.has(c.id)).map((c) => c.id);
      onStarted(await webqaStart(windowId, target.id, ids));
      onClose();
    } catch (err) {
      setError(errorText(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog
      title="Start web QA pass"
      onClose={onClose}
      footer={
        <div className="button-row">
          <button type="button" className="primary-button" disabled={busy || reason !== null} title={reason ?? undefined} onClick={() => void start()}>
            Start pass
          </button>
          <button type="button" className="secondary-button" onClick={onClose}>
            Cancel
          </button>
        </div>
      }
    >
      <p className="hint">
        Tries each ticked charter on the target's site in a headless browser, signed in with the test account. Nothing is sent to ClickUp
        until you tick findings.
      </p>
      {error && <p className="error">{error}</p>}
      {targets && targets.length === 0 && <p className="hint">No targets yet. Add one in Settings › Web QA.</p>}
      {targets && targets.length > 0 && (
        <label className="field-label">
          Target
          <select className="text-input" aria-label="Web QA target" value={targetId} onChange={(e) => setTargetId(e.target.value)}>
            {targets.map((t) => (
              <option key={t.id} value={t.id}>
                {t.name}
              </option>
            ))}
          </select>
        </label>
      )}
      {target && (
        <ul className="account-list">
          {target.charters.map((c) => (
            <li key={c.id} className="account-row">
              <label className="check-label">
                <input
                  type="checkbox"
                  checked={ticked.has(c.id)}
                  onChange={(e) =>
                    setTicked((now) => {
                      const next = new Set(now);
                      if (e.target.checked) next.add(c.id);
                      else next.delete(c.id);
                      return next;
                    })
                  }
                />
                {c.title}
                {c.mayChangeData ? " (may change data)" : ""}
              </label>
            </li>
          ))}
        </ul>
      )}
      {reason && <p className="hint">{reason}</p>}
    </Dialog>
  );
}
