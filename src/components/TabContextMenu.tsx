import { useEffect } from "react";
import { TerminalContextMenu, type ContextMenuItem } from "./TerminalContextMenu";

type Props = {
  x: number;
  y: number;
  items: ContextMenuItem[];
  onClose: () => void;
};

/**
 * The right-click menu of a tab (also opened from the keyboard): the
 * terminal menu's pattern. Escape closes it before the program or the page
 * sees the key, and so does focus moving outside the menu.
 */
export function TabContextMenu({ x, y, items, onClose }: Props) {
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      event.stopPropagation();
      onClose();
    };
    const onFocusIn = (event: FocusEvent) => {
      const target = event.target as Element | null;
      if (target?.closest?.(".terminal-menu, .terminal-menu-dismiss")) return;
      onClose();
    };
    window.addEventListener("keydown", onKey, true);
    document.addEventListener("focusin", onFocusIn);
    return () => {
      window.removeEventListener("keydown", onKey, true);
      document.removeEventListener("focusin", onFocusIn);
    };
  }, [onClose]);

  return (
    <>
      <TerminalContextMenu
        x={x}
        y={y}
        items={items.map((item) => ({
          ...item,
          onSelect: () => {
            onClose();
            item.onSelect();
          },
        }))}
      />
      <button
        type="button"
        className="terminal-menu-dismiss"
        aria-label="Close menu"
        data-own-escape=""
        onClick={onClose}
      />
    </>
  );
}
