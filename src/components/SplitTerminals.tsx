import { useRef, useState } from "react";
import type { SplitDirection, TabSummary } from "../bridge";
import { Divider } from "./Divider";
import { TerminalView } from "./TerminalView";

/** The first pane's share stays within this, like the backend's. */
export const MIN_RATIO = 0.2;
export const MAX_RATIO = 0.8;

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

  // The share a pointer at this position means: along the split's axis, within the box.
  const measure = (x: number, y: number) => {
    const rect = box.current?.getBoundingClientRect();
    if (!rect || !rect.width || !rect.height) return null;
    return horizontal ? (x - rect.left) / rect.width : (y - rect.top) / rect.height;
  };

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
      <Divider
        className="split-divider"
        orientation={horizontal ? "vertical" : "horizontal"}
        label="Resize panes"
        value={ratio}
        min={MIN_RATIO}
        max={MAX_RATIO}
        defaultValue={0.5}
        measure={measure}
        onLive={setLive}
        onCommit={onRatio}
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
