import { useEffect, useState } from "react";
import {
  activityText,
  costText,
  nudgeTab,
  observeTab,
  tokensText,
  type TabObservation,
} from "../run/observe";
import { errorText } from "./InlineNotice";

const POLL_MS = 3000;

type ViewProps = {
  seen: TabObservation;
  nudging: boolean;
  onNudge: () => void;
  onResume: () => void;
};

/** P3b in the run panel: tab state, watchdog, context meter, sub-agents, cost/time. */
export function ObserveView({ seen, nudging, onNudge, onResume }: ViewProps) {
  const [confirming, setConfirming] = useState(false);
  const activity = activityText(seen.hook?.activity);
  // Never type into a tab that may show a permission menu.
  const canNudge = seen.hook?.activity !== "needs-you";
  const nudge = () => {
    if (!seen.hooksActive) {
      setConfirming(true);
      return;
    }
    onNudge();
  };
  const usage = seen.usage;
  const subs = seen.subagents.slice(-8).reverse();
  return (
    <section className="run-observe" aria-label="Session">
      {activity && (
        <p className={`run-activity run-activity-${seen.hook?.activity}`} data-testid="tab-activity">
          Claude: {activity}
          {seen.hook && seen.hook.subagentsRunning > 0 && ` · ${seen.hook.subagentsRunning} sub-agents running`}
        </p>
      )}
      {seen.waiting && (
        <p className="hint warn" role="status" data-testid="run-waiting">
          Waiting for you for {seen.waiting.minutes} min.
        </p>
      )}
      {!seen.hooksInstalled && (
        <p className="hint" data-testid="hooks-missing">
          Hook status unavailable (dcv2-hook not found, or the tab started before this app run).
        </p>
      )}
      {seen.stall && (
        <div className="run-stall" role="status" data-testid="run-stall">
          <p className="hint warn">No progress for {seen.stall.minutes} min.</p>
          <div className="button-row">
            {canNudge && !confirming && (
              <button type="button" className="secondary-button" disabled={nudging} onClick={nudge}>
                Nudge
              </button>
            )}
            {confirming && (
              <>
                <span>Claude's state is unknown; a permission menu may be open. Type anyway?</span>
                <button
                  type="button"
                  className="secondary-button"
                  onClick={() => {
                    setConfirming(false);
                    onNudge();
                  }}
                >
                  Type it
                </button>
                <button type="button" className="secondary-button" onClick={() => setConfirming(false)}>
                  Cancel
                </button>
              </>
            )}
            <button type="button" className="secondary-button" onClick={onResume}>
              Resume
            </button>
          </div>
          <p className="hint">Nudge types “continue from status.json” into the tab. Resume starts the run again in a new tab (stop Claude in this tab first).</p>
        </div>
      )}
      {seen.context && (
        <p className={seen.context.warn ? "hint warn" : "hint"} data-testid="context-meter">
          Context {seen.context.percent.toFixed(0)}% ({tokensText(seen.context.used)} of {tokensText(seen.context.window)})
          {seen.context.warn && " — over 60%: consider /compact in the tab"}
        </p>
      )}
      {subs.length > 0 && (
        <ul className="run-subagents">
          {subs.map((sub) => (
            <li key={sub.toolUseId}>
              {sub.agentType.replace(/^dcv2:/, "")}
              {sub.description ? ` · ${sub.description}` : ""} — {sub.status}
            </li>
          ))}
        </ul>
      )}
      {usage && usage.messages > 0 && (
        <details className="run-cost" data-testid="run-cost">
          <summary>
            This session: {tokensText(usage.totalTokens)} tokens · {costText(usage.totalCostUsd)} estimate ·{" "}
            {usage.minutes.toFixed(0)} min
          </summary>
          <table>
            <thead>
              <tr>
                <th>Stage</th>
                <th>Tokens</th>
                <th>$ (estimate)</th>
                <th>Min</th>
              </tr>
            </thead>
            <tbody>
              {usage.stages.map((line) => (
                <tr key={line.name}>
                  <td>{line.name}</td>
                  <td>{tokensText(line.totalTokens)}</td>
                  <td>{costText(line.costUsd)}</td>
                  <td>{line.minutes.toFixed(1)}</td>
                </tr>
              ))}
            </tbody>
          </table>
          <p className="hint">
            By agent:{" "}
            {usage.agents.map((a) => `${a.name} ${tokensText(a.totalTokens)} ${costText(a.costUsd)}`).join(" · ")}
          </p>
          <p className="hint">
            These figures are for this tab's current session only. Resume or /clear starts a new session with its own
            figures.
          </p>
          <p className="hint">Estimate at API list prices; a subscription is not billed per token.</p>
        </details>
      )}
    </section>
  );
}

/** Polls the tab's session and shows [`ObserveView`]. */
export function ObserveSection({ tabId, onResume }: { tabId: string; onResume: () => void }) {
  const [seen, setSeen] = useState<TabObservation | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [nudging, setNudging] = useState(false);

  useEffect(() => {
    let stopped = false;
    let timer: number | undefined;
    const tick = () => {
      observeTab(tabId)
        .then(
          (next) => {
            setSeen(next);
            setProblem(null);
          },
          (err: unknown) => setProblem(errorText(err)),
        )
        .finally(() => {
          if (!stopped) timer = window.setTimeout(tick, POLL_MS);
        });
    };
    tick();
    return () => {
      stopped = true;
      window.clearTimeout(timer);
    };
  }, [tabId]);

  const nudge = () => {
    setNudging(true);
    nudgeTab(tabId)
      .catch((err: unknown) => setProblem(errorText(err)))
      .finally(() => setNudging(false));
  };

  return (
    <>
      {seen && <ObserveView seen={seen} nudging={nudging} onNudge={nudge} onResume={onResume} />}
      {problem && <p className="hint warn">{problem}</p>}
    </>
  );
}
