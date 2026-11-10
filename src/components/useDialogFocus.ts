import { useEffect, type RefObject } from "react";

/** What a dialog focuses first: its first field or button (not the corner close button). */
export const DIALOG_FIRST_FOCUS = "input, select, button:not(.dialog-close)";

/**
 * Focus moves into the container on mount (the first match of `first`, else the container itself) and goes back
 * to the element that had it before when the container unmounts. Shared by `Dialog` and the Settings page.
 */
export function useDialogFocus(ref: RefObject<HTMLElement | null>, first: string | null = DIALOG_FIRST_FOCUS) {
  useEffect(() => {
    const before = document.activeElement as HTMLElement | null;
    const target = first ? ref.current?.querySelector<HTMLElement>(first) : null;
    (target ?? ref.current)?.focus();
    return () => before?.focus?.();
    // Mount and unmount only.
  }, []);
}
