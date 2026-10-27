import { useRef, useState } from "react";
import type { SplitDirection, TabSummary } from "../bridge";
import { TerminalView } from "./TerminalView";

/** The first pane's share stays within this, like the backend's. */
export const MIN_RATIO = 0.2;
export const MAX_RATIO = 0.8;
const STEP = 0.05;

export function clampRatio(ratio: number): number {
  if (!Number.isFinite(ratio)) return 0.5;
  return Math.min(MAX_RATIO, Math.max(MIN_RATIO, ratio));
}

/** The second pane as a tab the terminal view can show: a Shell with its own id and folder. */
export function paneTab(tab: TabSummary): TabSummary {
  const split = tab.split!;
  return {
    ...tab,
    id: split.id,
    label: `Shell · ${split.cwd.split(/[\\/]/).filter(Boolean).pop() ?? split.cwd}`,
    kind: "shell",
    cwd: split.cwd,
    folderStatus: split.folderStatus,
    hasSession: false,
    runDir: null,
    role: null,
    split: null,
  };
}

type Props = {
  tab: TabSummary;
  fontSize: number;
  /** The tab is the active one and no dialog holds the keyboard. */
  autoFocus: boolean;
  /** Which pane the keyboard is in (1 is the tab's own terminal). */
  focusedPane: 1 | 2;
  onPaneFocus: (pane: 1 | 2) => void;
  onFolderChanged: () => void;
  /** The divider was released at this share of the first pane. */
  onRatio: (ratio: number) => void;
};

/**
 * A split tab: the tab's own terminal and its second Shell pane with a
 * divider between them (drag it, or focus it and use the arrow keys;
 * double-click resets to half). `right` puts the second pane beside the
 * first, `down` below it.
 */
export function SplitTerminals({ tab, fontSize, autoFocus, focusedPane, onPaneFocus, onFolderChanged, onRatio }: Props) {
  const split = tab.split!;
  const direction: SplitDirection = split.direction;
  const horizontal = direction === "right";
  const [live, setLive] = useState<number | null>(null);
  const box = useRef<HTMLDivElement>(null);
  const ratio = clampRatio(live ?? split.ratio);

  const drag = (event: React.PointerEvent<HTMLDivElement>) => {
    event.preventDefault();
    const target = event.currentTarget;
    target.setPointerCapture?.(event.pointerId);
    let last = ratio;
    const move = (e: PointerEvent) => {
      const rect = box.current?.getBoundingClientRect();
      if (!rect || !rect.width || !rect.height) return;
      last = clampRatio(horizontal ? (e.clientX - rect.left) / rect.width : (e.clientY - rect.top) / rect.height);
      setLive(last);
    };
    const stop = () => {
      target.removeEventListener("pointermove", move);
      target.removeEventListener("pointerup", stop);
      target.removeEventListener("pointercancel", stop);
      setLive(null);
      onRatio(last);
    };
    target.addEventListener("pointermove", move);
    target.addEventListener("pointerup", stop);
    target.addEventListener("pointercancel", stop);
  };

  const nudge = (delta: number) => onRatio(clampRatio(ratio + delta));

  return (
    <div ref={box} className={`split split-${direction}`} data-testid="split">
      <div
        className={`split-pane${focusedPane === 1 ? " split-pane-focused" : ""}`}
        style={{ flex: `${ratio} 1 0` }}
        data-testid="pane-1"
        onFocusCapture={() => onPaneFocus(1)}
        onMouseDownCapture={() => onPaneFocus(1)}
      >
        <TerminalView
          key={tab.id}
          tab={tab}
          fontSize={fontSize}
          autoFocus={autoFocus && focusedPane === 1}
          onFolderChanged={onFolderChanged}
        />
      </div>
      <div
        className="split-divider"
        role="separator"
        aria-orientation={horizontal ? "vertical" : "horizontal"}
        aria-label="Resize panes"
        aria-valuenow={Math.round(ratio * 100)}
        aria-valuemin={Math.round(MIN_RATIO * 100)}
        aria-valuemax={Math.round(MAX_RATIO * 100)}
        tabIndex={0}
        onPointerDown={drag}
        onDoubleClick={() => onRatio(0.5)}
        onKeyDown={(event) => {
          const less = horizontal ? "ArrowLeft" : "ArrowUp";
          const more = horizontal ? "ArrowRight" : "ArrowDown";
          if (event.key === less) nudge(-STEP);
          else if (event.key === more) nudge(STEP);
          else return;
          event.preventDefault();
        }}
      />
      <div
        className={`split-pane${focusedPane === 2 ? " split-pane-focused" : ""}`}
        style={{ flex: `${1 - ratio} 1 0` }}
        data-testid="pane-2"
        onFocusCapture={() => onPaneFocus(2)}
        onMouseDownCapture={() => onPaneFocus(2)}
      >
        <TerminalView
          key={split.id}
          tab={paneTab(tab)}
          fontSize={fontSize}
          autoFocus={autoFocus && focusedPane === 2}
          onFolderChanged={onFolderChanged}
        />
      </div>
    </div>
  );
}
