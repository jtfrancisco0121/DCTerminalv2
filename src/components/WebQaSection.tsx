import { useCallback, useEffect, useState } from "react";
import { getSettings } from "../bridge";
import {
  resumeRun,
  runQaImage,
  webqaApply,
  webqaApprove,
  webqaOverview,
  webqaReleaseOpen,
  type PassView,
  type StartedRun,
  type WebQaFinding,
} from "../run/api";
import { comparisonBadge, webqaResultTone } from "../run/words";
import { Dialog } from "./Dialog";
import { errorText } from "./InlineNotice";
import { Pill } from "./Pill";
import { WebQaStartDialog } from "./WebQaStartDialog";

const POLL_MS = 5000;

/** A charter's screenshot: a thumbnail of `webqa/shots/<name>.png`, or "Screenshot missing". */
function Shot({ pass, shot }: { pass: PassView; shot: string | null }) {
  const [url, setUrl] = useState<string | null | undefined>(undefined);
  useEffect(() => {
    if (!shot || !pass.runDir) {
      setUrl(null);
      return;
    }
    let live = true;
    void runQaImage(pass.runDir, shot.replace(/\.png$/, "")).then(
      (data) => live && setUrl(data),
      () => live && setUrl(null),
    );
    return () => {
      live = false;
    };
  }, [pass.runDir, shot]);
  if (url === undefined) return null;
  return url ? <img className="webqa-shot" src={url} alt="Screenshot of the charter" /> : <span className="hint">Screenshot missing</span>;
}

/** Why "Create tickets" is off, or null. */
export function ticketsReason(pass: PassView, ticked: number): string | null {
  if (!pass.clickupListId) return "Add a ClickUp list to this target in Settings › Web QA.";
  return ticked === 0 ? "Tick at least one finding." : null;
}

function Detail({ pass, windowId, onChanged }: { pass: PassView; windowId: string; onChanged: () => void }) {
  const [ticked, setTicked] = useState<Set<string>>(new Set());
  const [open, setOpen] = useState<Set<string>>(new Set());
  const [confirm, setConfirm] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const first = pass.comparison?.first === true;
  const reason = ticketsReason(pass, ticked.size);
  const index = (f: WebQaFinding) => pass.findings.indexOf(f);
  const sent = (f: WebQaFinding) => pass.posted[`create:${index(f)}`];

  const send = async () => {
    setBusy(true);
    setError(null);
    try {
      await webqaApprove(pass.passId, [...ticked]);
      const outcomes = await webqaApply(windowId, pass.passId);
      const bad = outcomes.find((o) => o.result !== "posted");
      if (bad) setError(`${bad.item}: ${bad.message}`);
      setTicked(new Set());
      setConfirm(false);
      onChanged();
    } catch (err) {
      setError(errorText(err));
      setConfirm(false);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="webqa-detail">
      {first && <p className="hint">First pass: nothing to compare yet.</p>}
      <table className="webqa-rows">
        <tbody>
          {pass.rows.map((row) => {
            const badge = first ? null : comparisonBadge(row.comparison);
            return (
              <tr key={row.charter}>
                <td>{row.title}</td>
                <td>
                  <Pill tone={webqaResultTone(row.result)}>{row.result}</Pill> {row.reason}
                </td>
                <td>{badge && <Pill tone={badge.tone}>{badge.text}</Pill>}</td>
                <td>
                  <Shot pass={pass} shot={row.shot} />
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
      {pass.findings.length > 0 && <h4>Findings</h4>}
      <ul className="account-list">
        {pass.findings.map((f) => {
          const task = sent(f);
          const badge = first ? null : comparisonBadge(f.comparison);
          return (
            <li key={f.key} className="account-row">
              <span className="field-row">
                <label className="check-label">
                  <input
                    type="checkbox"
                    aria-label={f.title}
                    disabled={task?.result === "posted" || task?.result === "maybe" || task?.result === "sending"}
                    checked={ticked.has(f.key)}
                    onChange={(e) =>
                      setTicked((now) => {
                        const next = new Set(now);
                        if (e.target.checked) next.add(f.key);
                        else next.delete(f.key);
                        return next;
                      })
                    }
                  />
                  {f.title}
                </label>
                <Pill tone={f.severity === "high" ? "danger" : "warn"}>{f.severity}</Pill>
                {badge && <Pill tone={badge.tone}>{badge.text}</Pill>}
                {task?.result === "posted" && <Pill tone="success">Created</Pill>}
                {task && task.result !== "posted" && <Pill tone="warn">{task.result}</Pill>}
                <button
                  type="button"
                  className="secondary-button"
                  aria-expanded={open.has(f.key)}
                  onClick={() =>
                    setOpen((now) => {
                      const next = new Set(now);
                      if (!next.delete(f.key)) next.add(f.key);
                      return next;
                    })
                  }
                >
                  {open.has(f.key) ? "Hide" : "Show"}
                </button>
              </span>
              {open.has(f.key) && (
                <div className="hint">
                  <p>Steps: {f.steps}</p>
                  <p>Expected: {f.expected}</p>
                  <p>Seen: {f.seen}</p>
                  <Shot pass={pass} shot={f.shot} />
                </div>
              )}
            </li>
          );
        })}
      </ul>
      {error && <p className="error">{error}</p>}
      {pass.findings.length > 0 && (
        <button type="button" className="primary-button" disabled={busy || reason !== null} title={reason ?? undefined} onClick={() => setConfirm(true)}>
          Create tickets
        </button>
      )}
      {confirm && (
        <Dialog
          title="Create ClickUp tasks"
          onClose={() => setConfirm(false)}
          footer={
            <div className="button-row">
              <button type="button" className="primary-button" disabled={busy} onClick={() => void send()}>
                Create
              </button>
              <button type="button" className="secondary-button" onClick={() => setConfirm(false)}>
                Cancel
              </button>
            </div>
          }
        >
          <p>
            Create {ticked.size} ClickUp {ticked.size === 1 ? "task" : "tasks"} in {pass.clickupListId}?
          </p>
        </Dialog>
      )}
    </div>
  );
}

function PassRow({ pass, windowId, onRerun, onOpenTab, onChanged }: { pass: PassView; windowId: string; onRerun: (p: PassView) => void; onOpenTab: (p: PassView) => void; onChanged: () => void }) {
  const [open, setOpen] = useState(false);
  const failing = pass.rows.filter((r) => r.result !== "PASS").length;
  return (
    <li className="account-row webqa-pass">
      <span className="field-row">
        <strong>{pass.targetName ?? pass.passId}</strong>
        <span className="hint">{pass.createdAt ?? ""}</span>
        {pass.running ? (
          <span className="hint" role="status">
            <span className="spinner" aria-hidden="true" /> Running…
          </span>
        ) : pass.result ? (
          <Pill tone={webqaResultTone(pass.result)}>
            {pass.result === "pass" ? "Passed" : pass.result === "fail" ? `${failing} failed` : "Could not run"}
          </Pill>
        ) : null}
        {pass.running && pass.runDir && (
          <button type="button" className="secondary-button" onClick={() => onOpenTab(pass)}>
            Go to tab
          </button>
        )}
        {pass.rows.length > 0 && (
          <button type="button" className="secondary-button" aria-expanded={open} onClick={() => setOpen((o) => !o)}>
            {open ? "Hide results" : "Show results"}
          </button>
        )}
        {!pass.running && !pass.release && pass.targetId && (
          <button type="button" className="secondary-button" onClick={() => onRerun(pass)}>
            Rerun
          </button>
        )}
      </span>
      {pass.problem && <p className="error">{pass.problem}</p>}
      {open && <Detail pass={pass} windowId={windowId} onChanged={onChanged} />}
    </li>
  );
}

/**
 * The Runs view's Web QA section: the passes (running, finished, or not valid), each with its result rows, the comparison
 * with the pass before it and its findings, which become ClickUp tasks only when ticked and confirmed.
 */
export function WebQaSection({ windowId, onStarted }: { windowId: string; onStarted: (run: StartedRun) => void }) {
  const [hasTargets, setHasTargets] = useState(true);
  const [passes, setPasses] = useState<PassView[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [start, setStart] = useState<{ target: string | null } | null>(null);
  const [releaseAsk, setReleaseAsk] = useState(false);
  const [version, setVersion] = useState("");
  const [release, setRelease] = useState<PassView | null>(null);

  const load = useCallback(
    () =>
      webqaOverview().then(
        (list) => setPasses(list),
        (err: unknown) => setError(errorText(err)),
      ),
    [],
  );
  useEffect(() => {
    void getSettings().then(
      (view) => setHasTargets((view.webQa?.targets ?? []).length > 0),
      () => {},
    );
  }, []);
  useEffect(() => {
    void load();
    const timer = setInterval(() => void load(), POLL_MS);
    return () => clearInterval(timer);
  }, [load]);

  const openTab = (pass: PassView) => {
    if (pass.runDir) void resumeRun(windowId, pass.runDir).then(onStarted, (err: unknown) => setError(errorText(err)));
  };
  const all = release ? [release, ...passes] : passes;

  return (
    <section className="runs-section" aria-label="Web QA">
      <h3>Web QA</h3>
      <div className="button-row">
        <button
          type="button"
          className="secondary-button"
          disabled={!hasTargets}
          title={hasTargets ? undefined : "Add a target in Settings › Web QA first."}
          onClick={() => setStart({ target: null })}
        >
          Start web QA pass…
        </button>
        <button type="button" className="secondary-button" onClick={() => setReleaseAsk((o) => !o)}>
          Open release QA results…
        </button>
      </div>
      {!hasTargets && <p className="hint">Add a target in Settings › Web QA first.</p>}
      {releaseAsk && (
        <div className="field-row">
          <input className="text-input text-input-narrow" aria-label="Release version" placeholder="0.10.0" value={version} onChange={(e) => setVersion(e.target.value)} />
          <button
            type="button"
            className="secondary-button"
            disabled={!version.trim()}
            onClick={() =>
              void webqaReleaseOpen(version.trim()).then(
                (p) => (setRelease(p), setError(null), setReleaseAsk(false)),
                (err: unknown) => setError(errorText(err)),
              )
            }
          >
            Open
          </button>
        </div>
      )}
      {error && <p className="error">{error}</p>}
      {all.length === 0 && <p className="hint">No web QA passes yet.</p>}
      <ul className="account-list">
        {all.map((pass) => (
          <PassRow
            key={pass.passId}
            pass={pass}
            windowId={windowId}
            onRerun={(p) => setStart({ target: p.targetId })}
            onOpenTab={openTab}
            onChanged={() => void load()}
          />
        ))}
      </ul>
      {start && <WebQaStartDialog windowId={windowId} initialTarget={start.target} onClose={() => setStart(null)} onStarted={onStarted} />}
    </section>
  );
}
