import { useEffect, useId, useMemo, useRef, useState } from "react";
import { reportError } from "../uiErrors";
import { Dialog } from "./Dialog";

/** The groups, in the order they tie-break and list in. */
export const PALETTE_GROUPS = ["Tabs", "Runs", "Roles", "Settings"] as const;
export type PaletteGroup = (typeof PALETTE_GROUPS)[number];

export type PaletteItem = {
  id: string;
  group: PaletteGroup;
  label: string;
  /** Secondary text (a folder, a stage). */
  detail?: string;
  /** Runs the item through the same handler the rest of the UI uses. */
  run: () => void | Promise<void>;
};

/** At most this many rows are listed. */
export const MAX_ROWS = 50;

/** 0: the label starts with the query; 1: a word of it does; 2: another subsequence match; null: no match. */
export function matchRank(label: string, query: string): number | null {
  const text = label.toLowerCase();
  const needle = query.trim().toLowerCase();
  if (!needle) return 2;
  if (text.startsWith(needle)) return 0;
  let at = 0;
  for (const ch of needle) {
    at = text.indexOf(ch, at);
    if (at === -1) return null;
    at += 1;
  }
  // A word start: the query is a prefix of some word, or of the label's initials.
  const words = text.split(/[^a-z0-9]+/).filter(Boolean);
  if (words.some((word) => word.startsWith(needle)) || words.map((w) => w[0]).join("").startsWith(needle)) return 1;
  return 2;
}

/**
 * Items whose label matches `query` as a case-insensitive subsequence,
 * ranked prefix, word start, other; ties keep the group order, then the
 * given order. At most [`MAX_ROWS`].
 */
export function rankItems(items: readonly PaletteItem[], query: string): PaletteItem[] {
  return items
    .map((item, index) => ({ item, index, rank: matchRank(item.label, query) }))
    .filter((entry): entry is { item: PaletteItem; index: number; rank: number } => entry.rank !== null)
    .sort(
      (a, b) =>
        a.rank - b.rank ||
        PALETTE_GROUPS.indexOf(a.item.group) - PALETTE_GROUPS.indexOf(b.item.group) ||
        a.index - b.index,
    )
    .slice(0, MAX_ROWS)
    .map((entry) => entry.item);
}

type Props = {
  items: readonly PaletteItem[];
  onClose: () => void;
};

/**
 * Cmd+K over any screen: type to filter this window's tabs, unfinished runs,
 * roles and Settings sections; Enter runs the active row through the same
 * handler as the rest of the UI. A failure is reported and closes it.
 */
export function CommandPalette({ items, onClose }: Props) {
  const [query, setQuery] = useState("");
  const [active, setActive] = useState(0);
  const listId = useId();
  const rows = useMemo(() => rankItems(items, query), [items, query]);
  const activeRef = useRef<HTMLLIElement | null>(null);
  const at = rows.length === 0 ? -1 : Math.min(active, rows.length - 1);

  useEffect(() => {
    activeRef.current?.scrollIntoView?.({ block: "nearest" });
  }, [at]);

  const run = (item: PaletteItem) => {
    Promise.resolve()
      .then(() => item.run())
      .catch((err: unknown) => reportError(`Command palette: ${item.label}`, err, { notify: true }))
      .finally(onClose);
  };

  return (
    <Dialog title="Command palette" onClose={onClose} bare className="palette-dialog" backdropClass="palette-backdrop">
      <input
        className="text-input palette-input"
        role="combobox"
        aria-expanded="true"
        aria-controls={listId}
        aria-activedescendant={at >= 0 ? `${listId}-${at}` : undefined}
        aria-label="Search tabs, runs, roles and settings"
        placeholder="Search tabs, runs, roles and settings"
        value={query}
        onChange={(event) => {
          setQuery(event.target.value);
          setActive(0);
        }}
        onKeyDown={(event) => {
          if (event.key === "ArrowDown" || event.key === "ArrowUp") {
            event.preventDefault();
            if (rows.length === 0) return;
            const step = event.key === "ArrowDown" ? 1 : -1;
            setActive((at + step + rows.length) % rows.length);
          } else if (event.key === "Enter") {
            event.preventDefault();
            if (at >= 0) run(rows[at]);
          }
        }}
      />
      {rows.length === 0 ? (
        <p className="hint palette-empty">Nothing matches</p>
      ) : (
        <ul className="palette-list" role="listbox" id={listId} aria-label="Results">
          {rows.map((item, index) => (
            <li
              key={item.id}
              id={`${listId}-${index}`}
              ref={index === at ? activeRef : undefined}
              role="option"
              aria-selected={index === at}
              className={`palette-row${index === at ? " palette-row-active" : ""}`}
              onMouseMove={() => setActive(index)}
              onClick={() => run(item)}
            >
              <span className="palette-group">{item.group}</span>
              <span className="palette-label">{item.label}</span>
              {item.detail && <span className="hint palette-detail">{item.detail}</span>}
            </li>
          ))}
        </ul>
      )}
    </Dialog>
  );
}
