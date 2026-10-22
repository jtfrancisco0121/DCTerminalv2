import { useState } from "react";
import type { TabSummary } from "../bridge";

type Props = {
  tabs: TabSummary[];
  activeTabId: string | null;
  /** Tabs whose program has exited, shown dimmed. */
  exited: ReadonlySet<string>;
  onSelect: (tabId: string) => void;
  onClose: (tabId: string) => void;
  onRename: (tabId: string, label: string) => void;
  onNew: () => void;
};

/** The window's tabs: click to show, double-click to rename, × to close, + for a new one. */
export function TabBar({ tabs, activeTabId, exited, onSelect, onClose, onRename, onNew }: Props) {
  const [editing, setEditing] = useState<{ id: string; value: string } | null>(null);

  const commit = () => {
    if (!editing) return;
    const value = editing.value.trim();
    const tab = tabs.find((item) => item.id === editing.id);
    setEditing(null);
    if (value && tab && value !== tab.label) onRename(editing.id, value);
  };

  return (
    <div className="tab-bar" role="tablist" aria-label="Tabs">
      {tabs.map((tab) => {
        const active = tab.id === activeTabId;
        const classes = ["tab", active ? "tab-active" : "", exited.has(tab.id) ? "tab-exited" : ""]
          .filter(Boolean)
          .join(" ");
        return (
          <div key={tab.id} className={classes} title={tab.cwd}>
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
                onClick={() => onSelect(tab.id)}
                onDoubleClick={() => setEditing({ id: tab.id, value: tab.label })}
              >
                <span className={`tab-kind tab-kind-${tab.kind}`} aria-hidden>
                  {tab.kind === "claude" ? "✳" : "$"}
                </span>
                {tab.label}
              </button>
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
      <button type="button" className="tab-new" aria-label="New tab" title="New tab" onClick={onNew}>
        +
      </button>
    </div>
  );
}
