import { useEffect, useRef, useState, type ReactNode } from "react";
import {
  clampWidth,
  DEFAULT_WIDTH_REM,
  loadLayout,
  MAX_WIDTH_REM,
  MIN_WIDTH_REM,
  saveLayout,
  STEP_REM,
  type PanelLayout,
} from "../runPanelLayout";

type Props = {
  windowId: string;
  /** The run's stage in words, for the collapsed rail's tooltip. */
  stageLabel: string;
  /** The run waits for the user (an approval stop): the rail says so. */
  attention: boolean;
  /** The layout changed: the terminal beside the panel refits. */
  onLayoutChange?: () => void;
  children: ReactNode;
};

/**
 * The frame around the run panel: a 6px drag handle (pointer, or the arrow
 * keys; double-click resets), a collapse control, and a narrow rail when
 * collapsed. Width and state are kept per window.
 */
export function RunPanelShell({ windowId, stageLabel, attention, onLayoutChange, children }: Props) {
  const [layout, setLayout] = useState<PanelLayout>(() => loadLayout(windowId));
  const frame = useRef<HTMLDivElement>(null);
  const first = useRef(true);

  const update = (next: Partial<PanelLayout>) =>
    setLayout((current) => {
      const merged = { ...current, ...next };
      merged.width = clampWidth(merged.width);
      saveLayout(windowId, merged);
      return merged;
    });

  // The terminal beside the panel refits after each change (not on mount).
  const refit = useRef(onLayoutChange);
  refit.current = onLayoutChange;
  useEffect(() => {
    if (first.current) {
      first.current = false;
      return;
    }
    const id = window.requestAnimationFrame(() => refit.current?.());
    return () => window.cancelAnimationFrame(id);
  }, [layout.width, layout.collapsed]);

  const drag = (event: React.PointerEvent<HTMLDivElement>) => {
    event.preventDefault();
    const target = event.currentTarget;
    target.setPointerCapture?.(event.pointerId);
    const rem = parseFloat(window.getComputedStyle(document.documentElement).fontSize) || 16;
    const right = frame.current?.getBoundingClientRect().right ?? window.innerWidth;
    const move = (e: PointerEvent) => update({ width: (right - e.clientX) / rem });
    const stop = () => {
      target.removeEventListener("pointermove", move);
      target.removeEventListener("pointerup", stop);
      target.removeEventListener("pointercancel", stop);
    };
    target.addEventListener("pointermove", move);
    target.addEventListener("pointerup", stop);
    target.addEventListener("pointercancel", stop);
  };

  if (layout.collapsed) {
    return (
      <aside className="run-rail" aria-label="Run progress (collapsed)">
        <button
          type="button"
          className={`run-rail-button${attention ? " run-rail-attention" : ""}`}
          aria-label="Expand run panel"
          title={attention ? `${stageLabel}: waiting for you` : stageLabel}
          onClick={() => update({ collapsed: false })}
        >
          <span className="run-rail-dot" aria-hidden />
        </button>
      </aside>
    );
  }
  return (
    <div className="run-panel-frame" ref={frame} style={{ width: `${layout.width}rem` }}>
      <div
        className="run-panel-handle"
        role="separator"
        aria-orientation="vertical"
        aria-label="Resize run panel"
        aria-valuenow={Math.round(layout.width)}
        aria-valuemin={MIN_WIDTH_REM}
        aria-valuemax={MAX_WIDTH_REM}
        tabIndex={0}
        onPointerDown={drag}
        onDoubleClick={() => update({ width: DEFAULT_WIDTH_REM })}
        onKeyDown={(event) => {
          // The handle is on the panel's left edge: Left widens, Right narrows.
          if (event.key === "ArrowLeft") update({ width: layout.width + STEP_REM });
          else if (event.key === "ArrowRight") update({ width: layout.width - STEP_REM });
          else return;
          event.preventDefault();
        }}
      />
      <button
        type="button"
        className="run-panel-collapse"
        aria-label="Collapse run panel"
        title="Collapse run panel"
        onClick={() => update({ collapsed: true })}
      >
        ›
      </button>
      {children}
    </div>
  );
}
