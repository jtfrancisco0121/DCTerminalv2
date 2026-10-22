import { useLayoutEffect, useRef } from "react";
import { clampMenuPosition, nextMenuIndex } from "../terminal/menu";

export type ContextMenuItem = {
  id: string;
  label: string;
  /** Shown at the right, for example the keyboard shortcut. */
  shortcut?: string;
  disabled?: boolean;
  /** Draw a divider above this item. */
  divider?: boolean;
  onSelect: () => void;
};

type Props = {
  x: number;
  y: number;
  items: ContextMenuItem[];
};

/**
 * Right-click menu: kept inside the window, first item focused, arrow keys
 * move, Home and End jump. Escape is handled by the owner. The root is
 * `data-own-escape` so the app-wide turn cancel stands down; the owner's
 * full-screen dismiss button carries the same mark. There is no Tab trap: the
 * owner closes the menu when focus leaves the menu and that button.
 */
export function TerminalContextMenu({ x, y, items }: Props) {
  const ref = useRef<HTMLDivElement>(null);

  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const at = clampMenuPosition(
      { x, y },
      { width: el.offsetWidth, height: el.offsetHeight },
      { width: window.innerWidth, height: window.innerHeight },
    );
    el.style.left = `${at.x}px`;
    el.style.top = `${at.y}px`;
    el.querySelector<HTMLButtonElement>("button:not(:disabled)")?.focus();
  }, [x, y]);

  const move = (event: React.KeyboardEvent<HTMLDivElement>) => {
    const buttons = Array.from(
      ref.current?.querySelectorAll<HTMLButtonElement>("button[role='menuitem']") ?? [],
    );
    const enabled = buttons.map((button) => !button.disabled);
    const current = buttons.indexOf(document.activeElement as HTMLButtonElement);
    let next = -1;
    if (event.key === "ArrowDown") next = nextMenuIndex(current, 1, enabled);
    else if (event.key === "ArrowUp") next = nextMenuIndex(current < 0 ? 0 : current, -1, enabled);
    else if (event.key === "Home") next = enabled.indexOf(true);
    else if (event.key === "End") next = enabled.lastIndexOf(true);
    else return;
    event.preventDefault();
    event.stopPropagation();
    if (next >= 0) buttons[next]?.focus();
  };

  return (
    <div
      ref={ref}
      className="terminal-menu"
      style={{ left: x, top: y }}
      role="menu"
      // While open, Escape closes the menu, not a running turn.
      data-own-escape=""
      onKeyDown={move}
    >
      {items.map((item) => (
        <button
          key={item.id}
          type="button"
          role="menuitem"
          disabled={item.disabled}
          className={item.divider ? "terminal-menu-divided" : undefined}
          onClick={item.onSelect}
        >
          <span>{item.label}</span>
          {item.shortcut && <span className="terminal-menu-shortcut">{item.shortcut}</span>}
        </button>
      ))}
    </div>
  );
}
