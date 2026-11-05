import { useId, type KeyboardEvent } from "react";
import { KIND } from "../flight/types";
import type { ReplayEntry } from "../flight/types";
import { flightKindText } from "../run/words";

type Props = {
  entries: ReplayEntry[];
  /** The index of the Event the replay stands on. */
  cursor: number;
  onCursor: (index: number) => void;
  truncated?: boolean;
};

const WIDTH = 1000;

const clock = (at: string) => {
  const date = new Date(at);
  return Number.isNaN(date.getTime()) ? "--:--:--" : date.toLocaleTimeString(undefined, { hourCycle: "h23" });
};

/** "12:01:03 file change: Edit". */
export function entryText(e: ReplayEntry): string {
  return `${clock(e.at)} ${flightKindText(e.kind)}${e.name ? `: ${e.name}` : ""}${e.file ? ` (${e.file} file${e.file === 1 ? "" : "s"})` : ""}`;
}

/**
 * The replay track: one slider over every Event of the Trace (not only the loaded page). `file.change` ticks are tall;
 * a `check.end` tick takes its result's tone. Arrow keys step one Event, Home and End jump; a click on the track jumps.
 */
export function ReplayTrack({ entries, cursor, onCursor, truncated }: Props) {
  const label = useId();
  const last = Math.max(0, entries.length - 1);
  const x = (i: number) => (entries.length <= 1 ? 0 : (i / last) * WIDTH);
  const current = entries[cursor];

  const onKey = (event: KeyboardEvent<HTMLDivElement>) => {
    const to =
      event.key === "ArrowRight"
        ? Math.min(last, cursor + 1)
        : event.key === "ArrowLeft"
          ? Math.max(0, cursor - 1)
          : event.key === "Home"
            ? 0
            : event.key === "End"
              ? last
              : null;
    if (to === null) return;
    event.preventDefault();
    event.stopPropagation();
    if (to !== cursor) onCursor(to);
  };

  return (
    <div className="replay-track-wrap">
      <span id={label} className="replay-track-label">
        Replay
      </span>
      <div
        className="replay-track"
        role="slider"
        tabIndex={0}
        aria-labelledby={label}
        aria-orientation="horizontal"
        aria-valuemin={0}
        aria-valuemax={last}
        aria-valuenow={cursor}
        aria-valuetext={current ? entryText(current) : "no events"}
        onKeyDown={onKey}
        data-testid="replay-track"
        onClick={(event) => {
          const box = event.currentTarget.getBoundingClientRect();
          if (box.width <= 0 || entries.length === 0) return;
          const at = Math.round(((event.clientX - box.left) / box.width) * last);
          onCursor(Math.min(last, Math.max(0, at)));
        }}
      >
        <svg viewBox={`0 0 ${WIDTH} 24`} preserveAspectRatio="none" width="100%" height="24" aria-hidden="true" focusable="false">
          <line x1="0" x2={WIDTH} y1="12" y2="12" className="replay-axis" />
          {entries.map((e, i) => {
            const file = e.kind === KIND.FILE_CHANGE;
            const check = e.kind === KIND.CHECK_END;
            if (!file && !check) return null;
            const height = file ? 16 : 8;
            const tone = check ? (e.status === "pass" ? "tone-success" : e.status === "fail" || e.status === "error" ? "tone-danger" : "tone-muted") : "";
            return (
              <line
                key={e.id}
                x1={x(i)}
                x2={x(i)}
                y1={12 - height / 2}
                y2={12 + height / 2}
                className={`replay-tick${file ? " replay-tick-file" : " replay-tick-check"} ${tone}`.trim()}
                data-kind={e.kind}
              />
            );
          })}
          <line x1={x(cursor)} x2={x(cursor)} y1="0" y2="24" className="replay-cursor" />
        </svg>
      </div>
      <span className="replay-track-count" aria-hidden="true">
        {entries.length === 0 ? "0" : `${cursor + 1} / ${entries.length}`}
        {truncated ? "+" : ""}
      </span>
    </div>
  );
}
