import { useRef, useState } from "react";
import type { TabSummary } from "../bridge";
import { urgentActivity } from "../run/observe";
import { DRAG_THRESHOLD_PX, dropIndex, markerX } from "../tabDrag";
import { TabContextMenu } from "./TabContextMenu";
import type { ContextMenuItem } from "./TerminalContextMenu";

type Props = {
  tabs: TabSummary[];
  activeTabId: string | null;
  /** Tabs whose program has exited, shown dimmed. */
  exited: ReadonlySet<string>;
  onSelect: (tabId: string) => void;
  onClose: (tabId: string) => void;
  onRename: (tabId: string, label: string) => void;
  onNew: () => void;
  /** P3b: tab id → what its Claude does (`working`, `waiting`, `needs-you`). */
  activity?: Readonly<Record<string, string>>;
  /** Click on the dot of a plain Claude tab (no run): shows its usage. */
  onActivityClick?: (tabId: string) => void;
  /** A tab was dragged to this place among the window's tabs. */
  onMove?: (tabId: string, index: number) => void;
  /** Context menu: a new tab of the same kind in the same folder. */
  onDuplicate?: (tabId: string) => void;
  /** Context menu: show the tab's folder in the file manager. */
  onReveal?: (tabId: string) => void;
  /** Context menu: close these tabs (the owner asks first when a terminal runs). */
  onCloseMany?: (tabIds: string[]) => void;
  /** Context menu: split the tab (absent: the items are disabled). */
  onSplit?: (tabId: string, direction: "right" | "down") => void;
};

type MenuHandlers = Pick<Props, "onMove" | "onDuplicate" | "onReveal" | "onCloseMany" | "onSplit"> & {
  onRename: () => void;
};

/**
 * The items of a tab's context menu. Move left / right are the keyboard
 * alternative to dragging and are disabled at the ends; Close others and
 * Close to the right are disabled when no tab would close.
 */
export function tabMenuItems(tabs: readonly TabSummary[], tabId: string, handlers: MenuHandlers): ContextMenuItem[] {
  const at = tabs.findIndex((tab) => tab.id === tabId);
  const others = tabs.filter((tab) => tab.id !== tabId).map((tab) => tab.id);
  const right = at < 0 ? [] : tabs.slice(at + 1).map((tab) => tab.id);
  return [
    { id: "rename", label: "Rename", onSelect: handlers.onRename },
    {
      id: "duplicate",
      label: "Duplicate in same folder",
      disabled: !handlers.onDuplicate,
      onSelect: () => handlers.onDuplicate?.(tabId),
    },
    { id: "reveal", label: "Reveal folder", disabled: !handlers.onReveal, onSelect: () => handlers.onReveal?.(tabId) },
    {
      id: "move-left",
      label: "Move left",
      divider: true,
      disabled: at <= 0 || !handlers.onMove,
      onSelect: () => handlers.onMove?.(tabId, at - 1),
    },
    {
      id: "move-right",
      label: "Move right",
      disabled: at < 0 || at >= tabs.length - 1 || !handlers.onMove,
      onSelect: () => handlers.onMove?.(tabId, at + 1),
    },
    {
      id: "split-right",
      label: "Split right",
      divider: true,
      disabled: !handlers.onSplit || Boolean(tabs[at]?.split),
      onSelect: () => handlers.onSplit?.(tabId, "right"),
    },
    {
      id: "split-down",
      label: "Split down",
      disabled: !handlers.onSplit || Boolean(tabs[at]?.split),
      onSelect: () => handlers.onSplit?.(tabId, "down"),
    },
    {
      id: "close-others",
      label: "Close others",
      divider: true,
      disabled: others.length === 0 || !handlers.onCloseMany,
      onSelect: () => handlers.onCloseMany?.(others),
    },
    {
      id: "close-right",
      label: "Close to the right",
      disabled: right.length === 0 || !handlers.onCloseMany,
      onSelect: () => handlers.onCloseMany?.(right),
    },
  ];
}

/** A drag in progress: the tab, how far the pointer moved, and where it would land. */
type Drag = { id: string; startX: number; dx: number; active: boolean; index: number; marker: number | null; cancelled: boolean };

/** The dot's tooltip. */
export function activityTitle(activity: string): string {
  if (activity === "needs-you") return "Claude needs you";
  if (activity === "waiting") return "Claude finished its turn";
  return `Claude is ${activity}`;
}

/** The window's tabs: click to show, double-click to rename, × to close, + for a new one. */
export function TabBar({ tabs, activeTabId, exited, onSelect, onClose, onRename, onNew, activity, onActivityClick, onMove, onDuplicate, onReveal, onCloseMany, onSplit }: Props) {
  const [editing, setEditing] = useState<{ id: string; value: string } | null>(null);
  const [drag, setDrag] = useState<Drag | null>(null);
  const [menu, setMenu] = useState<{ tabId: string; x: number; y: number } | null>(null);
  const bar = useRef<HTMLDivElement>(null);
  const tabEls = useRef(new Map<string, HTMLDivElement>());
  // The click that ends a drag must not select the tab.
  const swallowClick = useRef(false);
  const tabsRef = useRef(tabs);
  tabsRef.current = tabs;
  const onMoveRef = useRef(onMove);
  onMoveRef.current = onMove;

  const beginDrag = (event: React.PointerEvent, id: string) => {
    if (event.button !== 0 || !onMove) return;
    let state: Drag = { id, startX: event.clientX, dx: 0, active: false, index: 0, marker: null, cancelled: false };
    const others = () => tabsRef.current.filter((tab) => tab.id !== id);
    const rects = () =>
      others().map((tab) => tabEls.current.get(tab.id)?.getBoundingClientRect() ?? { left: 0, right: 0 });
    const move = (e: PointerEvent) => {
      const dx = e.clientX - state.startX;
      if (!state.active && Math.abs(dx) < DRAG_THRESHOLD_PX) return;
      const boxes = rects();
      const index = dropIndex(
        boxes.map((box) => (box.left + box.right) / 2),
        e.clientX,
      );
      const edge = markerX(boxes, index);
      const barBox = bar.current?.getBoundingClientRect();
      state = {
        ...state,
        active: true,
        dx,
        index,
        marker: edge == null ? null : edge - (barBox?.left ?? 0) + (bar.current?.scrollLeft ?? 0),
      };
      if (!state.cancelled) setDrag(state);
    };
    const finish = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", finish);
      window.removeEventListener("keydown", key, true);
      if (state.active) {
        swallowClick.current = true;
        window.setTimeout(() => (swallowClick.current = false), 0);
      }
      setDrag(null);
      const from = tabsRef.current.findIndex((tab) => tab.id === id);
      if (state.active && !state.cancelled && state.index !== from) onMoveRef.current?.(id, state.index);
    };
    const key = (e: KeyboardEvent) => {
      if (e.key !== "Escape" || !state.active) return;
      e.preventDefault();
      e.stopPropagation();
      state = { ...state, cancelled: true };
      setDrag(null);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", finish);
    window.addEventListener("keydown", key, true);
  };

  const commit = () => {
    if (!editing) return;
    const value = editing.value.trim();
    const tab = tabs.find((item) => item.id === editing.id);
    setEditing(null);
    if (value && tab && value !== tab.label) onRename(editing.id, value);
  };

  return (
    <div className="tab-bar" role="tablist" aria-label="Tabs" ref={bar}>
      {tabs.map((tab) => {
        const active = tab.id === activeTabId;
        const dragging = drag?.active && drag.id === tab.id;
        const classes = ["tab", active ? "tab-active" : "", exited.has(tab.id) ? "tab-exited" : "", dragging ? "tab-dragging" : ""]
          .filter(Boolean)
          .join(" ");
        // A split tab's dot shows its most urgent pane.
        const shown = tab.split
          ? urgentActivity([activity?.[tab.id], activity?.[tab.split.id]])
          : activity?.[tab.id];
        const showDot = Boolean(shown) && shown !== "unknown";
        const clickableDot = Boolean(onActivityClick) && tab.kind === "claude" && !tab.role;
        return (
          <div
            key={tab.id}
            className={classes}
            title={tab.cwd}
            onContextMenu={(event) => {
              event.preventDefault();
              setMenu({ tabId: tab.id, x: event.clientX, y: event.clientY });
            }}
            style={dragging ? { transform: `translateX(${drag.dx}px)` } : undefined}
            ref={(el) => {
              if (el) tabEls.current.set(tab.id, el);
              else tabEls.current.delete(tab.id);
            }}
          >
            {editing?.id === tab.id ? (
              <input
                className="tab-rename"
                aria-label="Tab name"
                autoFocus
                value={editing.value}
                onChange={(event) => setEditing({ id: tab.id, value: event.target.value })}
                onBlur={commit}
                onKeyDown={(event) => {
                  if (event.key === "Enter") commit();
                  if (event.key === "Escape") setEditing(null);
                }}
              />
            ) : (
              <button
                type="button"
                role="tab"
                aria-selected={active}
                className="tab-label"
                onPointerDown={(event) => {
                  // Middle-click closes the tab (on release); no autoscroll.
                  if (event.button === 1) event.preventDefault();
                  beginDrag(event, tab.id);
                }}
                onAuxClick={(event) => {
                  if (event.button !== 1) return;
                  event.preventDefault();
                  onClose(tab.id);
                }}
                onKeyDown={(event) => {
                  // The keyboard way to the context menu.
                  if (event.key === "ContextMenu" || (event.key === "F10" && event.shiftKey)) {
                    event.preventDefault();
                    const box = event.currentTarget.getBoundingClientRect();
                    setMenu({ tabId: tab.id, x: box.left, y: box.bottom });
                  }
                }}
                onClick={() => {
                  if (swallowClick.current) return;
                  onSelect(tab.id);
                }}
                onDoubleClick={() => setEditing({ id: tab.id, value: tab.label })}
              >
                <span className={`tab-kind tab-kind-${tab.kind}`} aria-hidden>
                  {tab.kind === "claude" ? "✳" : "$"}
                </span>
                {tab.label}
                {showDot && !clickableDot && (
                  <span
                    className={`tab-activity tab-activity-${shown!}`}
                    title={activityTitle(shown!)}
                    aria-label={`Claude ${shown!}`}
                  />
                )}
              </button>
            )}
            {showDot && clickableDot && (
              <button
                type="button"
                className={`tab-activity tab-activity-button tab-activity-${shown!}`}
                title={activityTitle(shown!)}
                aria-label={`Claude ${shown!}: show usage`}
                onClick={() => onActivityClick!(tab.id)}
              />
            )}
            <button
              type="button"
              className="tab-close"
              aria-label={`Close ${tab.label}`}
              onClick={() => onClose(tab.id)}
            >
              ×
            </button>
          </div>
        );
      })}
      {menu && (
        <TabContextMenu
          x={menu.x}
          y={menu.y}
          onClose={() => setMenu(null)}
          items={tabMenuItems(tabs, menu.tabId, {
            onMove,
            onDuplicate,
            onReveal,
            onCloseMany,
            onSplit,
            onRename: () => {
              const target = tabs.find((item) => item.id === menu.tabId);
              if (target) setEditing({ id: target.id, value: target.label });
            },
          })}
        />
      )}
      {drag?.active && drag.marker != null && (
        <div className="tab-insert-marker" style={{ left: drag.marker }} data-testid="tab-insert-marker" aria-hidden />
      )}
      <button type="button" className="tab-new" aria-label="New tab" title="New tab" onClick={onNew}>
        +
      </button>
    </div>
  );
}
