import type { PointerEvent as ReactPointerEvent } from "react";

type Props = {
  /** The line's direction: `vertical` splits side-by-side panes (drag and arrow keys go left/right), `horizontal` stacked ones (up/down). */
  orientation: "vertical" | "horizontal";
  /** The divider's position as a share (0..1) of the container, from its start edge (left or top). */
  value: number;
  min: number;
  max: number;
  /** An arrow key moves the value this far (default 0.05). */
  step?: number;
  /** Double-click resets to this value. */
  defaultValue: number;
  label: string;
  className?: string;
  /** The share (0..1, from the start edge) a pointer at this client position means, or null when it cannot be measured. */
  measure: (clientX: number, clientY: number) => number | null;
  /** The value during a drag (the owner draws it live); null when the drag ends. */
  onLive?: (value: number | null) => void;
  /** The value chosen: a drag released, an arrow key, a double-click. The only call that should store anything. */
  onCommit: (value: number) => void;
};

const STEP = 0.05;

function clamp(value: number, min: number, max: number, fallback: number): number {
  if (!Number.isFinite(value)) return fallback;
  return Math.min(max, Math.max(min, value));
}

/**
 * A draggable separator between two panes: pointer capture with a live value that commits on release, the arrow keys
 * along its axis, double-click to reset, and the separator ARIA values in percent from the start edge. Shared by the
 * split terminals and the preview pane, so the two cannot behave differently.
 */
export function Divider({ orientation, value, min, max, step = STEP, defaultValue, label, className, measure, onLive, onCommit }: Props) {
  const vertical = orientation === "vertical";
  const at = (v: number) => clamp(v, min, max, defaultValue);

  const drag = (event: ReactPointerEvent<HTMLDivElement>) => {
    event.preventDefault();
    const target = event.currentTarget;
    target.setPointerCapture?.(event.pointerId);
    let last = at(value);
    const move = (e: PointerEvent) => {
      const share = measure(e.clientX, e.clientY);
      if (share === null) return;
      last = at(share);
      onLive?.(last);
    };
    const stop = () => {
      target.removeEventListener("pointermove", move);
      target.removeEventListener("pointerup", stop);
      target.removeEventListener("pointercancel", stop);
      onLive?.(null);
      onCommit(last);
    };
    target.addEventListener("pointermove", move);
    target.addEventListener("pointerup", stop);
    target.addEventListener("pointercancel", stop);
  };

  return (
    <div
      className={className}
      role="separator"
      aria-orientation={vertical ? "vertical" : "horizontal"}
      aria-label={label}
      aria-valuenow={Math.round(value * 100)}
      aria-valuemin={Math.round(min * 100)}
      aria-valuemax={Math.round(max * 100)}
      tabIndex={0}
      onPointerDown={drag}
      onDoubleClick={() => onCommit(at(defaultValue))}
      onKeyDown={(event) => {
        const less = vertical ? "ArrowLeft" : "ArrowUp";
        const more = vertical ? "ArrowRight" : "ArrowDown";
        if (event.key === less) onCommit(at(value - step));
        else if (event.key === more) onCommit(at(value + step));
        else return;
        event.preventDefault();
      }}
    />
  );
}
