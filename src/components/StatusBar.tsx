import { useRef, useState, useSyncExternalStore } from "react";
import type { ConfigDirInfo } from "../bridge";
import { subscribeUiErrors, uiErrors } from "../uiErrors";
import {
  formatReset,
  limitTone,
  meterValue,
  periodReadings,
  trendText,
  type LimitLine,
  type RateWindow,
  type Reading,
  type WindowHistory,
} from "../usage/limits";
import { usageTone } from "../run/words";
import { ErrorsPopover } from "./ErrorsPopover";
import { Meter } from "./Meter";

type Props = {
  /** This window's Claude account. Null until the window context loads. */
  account: { name: string; config: ConfigDirInfo } | null;
  /** The active tab's folder. */
  folder?: string | null;
  /** Claude 5h/7d line. Null hides it. */
  usage?: LimitLine | null;
  /** The limit windows and their saved readings: one meter each, with a sparkline on hover or focus. */
  windows?: RateWindow[];
  history?: WindowHistory[];
  /** A short notice (a failed action), with its dismiss. */
  message?: { text: string; onDismiss: () => void } | null;
  /** Tabs that need you (every window) and unfinished runs. Null hides the control. */
  attention?: { needsYou: number; activeRuns: number } | null;
  /** The attention control was clicked: open Active runs. */
  onAttention?: () => void;
  onNewWindow: () => void;
  onSettings: () => void;
};

/** The attention control's words: "2 need you · 3 runs", "3 runs"; null when there is nothing. */
export function attentionText(attention: { needsYou: number; activeRuns: number }): string | null {
  const parts: string[] = [];
  if (attention.needsYou > 0) parts.push(`${attention.needsYou} ${attention.needsYou === 1 ? "needs" : "need"} you`);
  if (attention.activeRuns > 0) parts.push(`${attention.activeRuns} ${attention.activeRuns === 1 ? "run" : "runs"}`);
  return parts.length ? parts.join(" · ") : null;
}

function folderName(path: string): string {
  const parts = path.split(/[\\/]/).filter(Boolean);
  return parts[parts.length - 1] ?? path;
}

/**
 * Why an account's folder will not work: one you set that does not exist.
 * The default `~/.claude` may not exist yet; Claude Code makes it.
 */
export function accountFolderWarning(config: ConfigDirInfo): string | null {
  if (config.exists || config.source === "default") return null;
  return `Claude config folder not found: ${config.path}`;
}

const SPARK_W = 140;
const SPARK_H = 36;

/** A sparkline of utilization (0-100) over time as an inline SVG polyline. */
function Sparkline({ readings, label }: { readings: Reading[]; label: string }) {
  const first = readings[0].atMs;
  const span = Math.max(readings[readings.length - 1].atMs - first, 1);
  const points = readings
    .map((r) => {
      const x = ((r.atMs - first) / span) * (SPARK_W - 4) + 2;
      const y = SPARK_H - 2 - (Math.min(100, Math.max(0, r.utilization ?? 0)) / 100) * (SPARK_H - 4);
      return `${x.toFixed(1)},${y.toFixed(1)}`;
    })
    .join(" ");
  return (
    <svg className="usage-spark-line" width={SPARK_W} height={SPARK_H} viewBox={`0 0 ${SPARK_W} ${SPARK_H}`} role="img" aria-label={label}>
      <polyline points={points} fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinejoin="round" strokeLinecap="round" />
    </svg>
  );
}

/** One window's meter button; hover or focus opens its sparkline, Escape or leaving closes it. */
function StatusMeter({ window, history, now }: { window: RateWindow; history: WindowHistory[]; now: number }) {
  const [open, setOpen] = useState(false);
  const value = meterValue(window, now);
  const label = `Claude ${window.label} ${value === null ? "unknown" : `${Math.round(value)}%`}`;
  const readings = periodReadings(window, history);
  const trend = trendText(window, history);
  const reset = formatReset(window.resetsAt);
  const id = `usage-spark-${window.rateLimitType}`;
  return (
    <span className="status-meter-wrap" onMouseEnter={() => setOpen(true)} onMouseLeave={() => setOpen(false)}>
      <button
        type="button"
        className="status-meter"
        aria-label={label}
        aria-describedby={open ? id : undefined}
        onFocus={() => setOpen(true)}
        onBlur={() => setOpen(false)}
        onKeyDown={(event) => {
          if (event.key === "Escape" && open) {
            event.stopPropagation();
            setOpen(false);
          }
        }}
      >
        <span className="status-meter-name" aria-hidden="true">
          {window.label}
        </span>
        <Meter value={value} tone={value === null ? "muted" : usageTone(limitTone(window))} label={label} className="status-meter-bar" />
      </button>
      {open && (
        <div className="usage-spark" id={id} role="tooltip">
          <div className="usage-spark-title">{label}</div>
          {readings.length >= 2 ? (
            <Sparkline readings={readings} label={`${window.label} usage over this period`} />
          ) : (
            <div>Not enough readings yet.</div>
          )}
          {trend && <div>{trend}</div>}
          {reset && <div>resets {reset}</div>}
        </div>
      )}
    </span>
  );
}

/**
 * One strip under the terminal: the window's account and its folder, the
 * active tab's folder, Claude usage, and the window and settings buttons.
 */
export function StatusBar({
  account,
  folder = null,
  usage = null,
  windows = [],
  history = [],
  message = null,
  attention = null,
  onAttention,
  onNewWindow,
  onSettings,
}: Props) {
  const errors = useSyncExternalStore(subscribeUiErrors, uiErrors);
  const [errorsOpen, setErrorsOpen] = useState(false);
  const countRef = useRef<HTMLButtonElement>(null);
  const closeErrors = () => {
    setErrorsOpen(false);
    countRef.current?.focus();
  };
  const attentionLabel = attention ? attentionText(attention) : null;
  const warning = account ? accountFolderWarning(account.config) : null;
  const now = Date.now();
  return (
    <footer className="status-bar" aria-label="Status bar">
      {account && (
        <span
          className={`status-bar-item status-account${warning ? " status-bar-warn" : ""}`}
          title={warning ?? `Claude account for this window\n${account.config.path}`}
          data-testid="status-account"
        >
          {account.name} · {account.config.display}
          {warning && " · folder not found"}
        </span>
      )}
      {folder && (
        <span className="status-bar-item status-bar-folder" title={folder}>
          {folderName(folder)}
        </span>
      )}
      {usage && (
        <span
          className={`status-bar-item status-usage status-usage-${usage.tone}`}
          title={usage.title}
          data-testid="status-usage"
        >
          {usage.text}
        </span>
      )}
      {windows.length > 0 && (
        <span className="status-meters" data-testid="status-meters">
          {windows.map((w) => (
            <StatusMeter key={w.rateLimitType} window={w} history={history} now={now} />
          ))}
        </span>
      )}
      {message && (
        <span className="status-bar-message status-bar-message-error" role="alert" title={message.text}>
          <span className="status-bar-message-text">{message.text}</span>
          <button
            type="button"
            className="status-bar-dismiss"
            aria-label={`Dismiss: ${message.text}`}
            onClick={message.onDismiss}
          >
            ×
          </button>
        </span>
      )}
      {attention && attentionLabel && (
        <button
          type="button"
          className={`status-bar-item status-attention${attention.needsYou > 0 ? " status-attention-needs" : ""}`}
          data-testid="status-attention"
          title="Tabs that need you and unfinished runs, in every window"
          onClick={onAttention}
        >
          {attentionLabel}
        </button>
      )}
      {errors.length > 0 && (
        <button
          ref={countRef}
          type="button"
          className="status-bar-item error-count"
          data-testid="error-count"
          aria-expanded={errorsOpen}
          title="Recent errors in this window"
          onClick={() => (errorsOpen ? closeErrors() : setErrorsOpen(true))}
        >
          {errors.length} {errors.length === 1 ? "error" : "errors"}
        </button>
      )}
      {errorsOpen && errors.length > 0 && <ErrorsPopover errors={errors} onClose={closeErrors} />}
      <span className="status-bar-spacer" />
      <button type="button" className="status-bar-button" onClick={onNewWindow} title="New window (⌘⇧N)">
        New window…
      </button>
      <button type="button" className="status-bar-button" aria-label="Settings" title="Settings (⌘,)" onClick={onSettings}>
        ⚙
      </button>
    </footer>
  );
}
