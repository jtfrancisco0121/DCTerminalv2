/** The run panel's width and collapsed state, kept per window in localStorage. */

export const MIN_WIDTH_REM = 16;
export const MAX_WIDTH_REM = 40;
export const DEFAULT_WIDTH_REM = 20;
export const STEP_REM = 1;

export type PanelLayout = { width: number; collapsed: boolean };

export const DEFAULT_LAYOUT: PanelLayout = { width: DEFAULT_WIDTH_REM, collapsed: false };

/** Within 16..40 rem; anything that is not a number is the default. */
export function clampWidth(rem: number): number {
  if (!Number.isFinite(rem)) return DEFAULT_WIDTH_REM;
  return Math.min(MAX_WIDTH_REM, Math.max(MIN_WIDTH_REM, rem));
}

export function layoutKey(windowId: string): string {
  return `dcv2.runPanel.${windowId}`;
}

/** The saved layout, or the default. Storage that throws or holds junk is ignored. */
export function loadLayout(windowId: string): PanelLayout {
  try {
    const raw = window.localStorage.getItem(layoutKey(windowId));
    if (!raw) return DEFAULT_LAYOUT;
    const parsed: unknown = JSON.parse(raw);
    if (!parsed || typeof parsed !== "object") return DEFAULT_LAYOUT;
    const { width, collapsed } = parsed as Record<string, unknown>;
    return {
      width: typeof width === "number" ? clampWidth(width) : DEFAULT_WIDTH_REM,
      collapsed: collapsed === true,
    };
  } catch {
    return DEFAULT_LAYOUT;
  }
}

export function saveLayout(windowId: string, layout: PanelLayout): void {
  try {
    window.localStorage.setItem(layoutKey(windowId), JSON.stringify(layout));
  } catch {
    // Storage is full or blocked: the layout just is not remembered.
  }
}
