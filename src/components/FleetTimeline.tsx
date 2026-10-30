import { useEffect, useState } from "react";
import { costText, layoutTimeline, minutesText, runTimeline, type Timeline } from "../run/timeline";
import { errorText } from "./InlineNotice";

const REFRESH_MS = 5000;

/** The run panel's "Agents" section: one lane per agent, approval waits shaded, totals for work, waiting and cost. */
export function FleetTimelineView({ timeline }: { timeline: Timeline }) {
  const layout = layoutTimeline(timeline);
  if (timeline.lanes.length === 0) return <p className="hint">No agents were recorded for this run yet.</p>;
  return (
    <div className="fleet" data-testid="fleet">
      <p className="fleet-totals" data-testid="fleet-totals">
        <span>Work {minutesText(timeline.workMinutes)}</span> · <span>Waiting for you {minutesText(timeline.waitMinutes)}</span> ·{" "}
        <span>{costText(timeline.costUsd)}</span>
      </p>
      <ol className="fleet-lanes">
        {layout.bars.map(({ lane, left, width, open, name }) => (
          <li key={lane.id} className="fleet-lane">
            <span className="fleet-label">{lane.label}</span>
            <span className="fleet-track">
              {layout.waits.map((w, i) => (
                <span key={i} className="fleet-wait" aria-hidden="true" title={w.name} style={{ left: `${w.left}%`, width: `${w.width}%` }} />
              ))}
              <span
                className={`fleet-bar fleet-bar-${lane.state}${open ? " fleet-bar-open" : ""}`}
                tabIndex={0}
                role="img"
                aria-label={name}
                title={name}
                style={{ left: `${left}%`, width: `${width}%` }}
              />
            </span>
            <span className="fleet-state">{lane.state === "unfinished" ? "unfinished" : lane.state}</span>
          </li>
        ))}
      </ol>
    </div>
  );
}

/** Reads the timeline now and again every few seconds while the run is not finished. */
export function FleetTimeline({ runDir, live }: { runDir: string; live: boolean }) {
  const [timeline, setTimeline] = useState<Timeline | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  useEffect(() => {
    let current = true;
    const read = () =>
      runTimeline(runDir).then(
        (next) => current && (setTimeline(next), setProblem(null)),
        (err: unknown) => current && setProblem(errorText(err)),
      );
    void read();
    const timer = live ? window.setInterval(() => void read(), REFRESH_MS) : undefined;
    return () => {
      current = false;
      if (timer !== undefined) window.clearInterval(timer);
    };
  }, [runDir, live]);
  return (
    <section className="run-agents" aria-label="Agents" data-testid="agents">
      <h3>Agents</h3>
      {problem && <p className="hint warn">{problem}</p>}
      {!timeline && !problem && <p className="hint">Reading the agents…</p>}
      {timeline && <FleetTimelineView timeline={timeline} />}
    </section>
  );
}
