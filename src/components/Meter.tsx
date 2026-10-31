import type { Tone } from "../run/words";

/**
 * A bar meter (CSS and an ARIA `meter`, no chart library). `value` is a
 * percent; `null` is unknown and draws a hatched empty bar, never a 0% fill.
 * `marker` is an optional threshold line.
 */
export function Meter({
  value,
  tone,
  label,
  marker,
  testId,
  className,
}: {
  value: number | null;
  tone: Tone;
  label: string;
  marker?: number | null;
  testId?: string;
  className?: string;
}) {
  const known = value !== null && Number.isFinite(value);
  const percent = known ? Math.min(100, Math.max(0, value)) : 0;
  const name = known || /unknown/i.test(label) ? label : `${label}: unknown`;
  return (
    <span
      role="meter"
      className={`meter tone-${known ? tone : "muted"}${known ? "" : " meter-unknown"}${className ? ` ${className}` : ""}`}
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={known ? percent : undefined}
      aria-label={name}
      data-testid={testId}
    >
      {known && <span className="meter-fill" style={{ width: `${percent}%` }} />}
      {marker != null && Number.isFinite(marker) && (
        <span className="meter-marker" style={{ left: `${Math.min(100, Math.max(0, marker))}%` }} />
      )}
    </span>
  );
}
