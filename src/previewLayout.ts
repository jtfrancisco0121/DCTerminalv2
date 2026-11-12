/** The preview pane's width share, kept per window in localStorage, and the pure fit that decides side or overlay. */

export const DEFAULT_PREVIEW_RATIO = 0.42;
export const MIN_PREVIEW_RATIO = 0.15;
export const MAX_PREVIEW_RATIO = 0.85;
/** The terminal area keeps at least this many columns while the preview is beside it. */
export const MIN_TERMINAL_COLS = 60;
/** Below this window width the preview is an overlay. */
export const OVERLAY_BELOW_PX = 1000;

export function previewKey(windowId: string): string {
  return `dcv2.preview.${windowId}`;
}

/** Within 0.15..0.85; anything that is not a number is the default. */
export function clampPreviewRatio(ratio: unknown): number {
  if (typeof ratio !== "number" || !Number.isFinite(ratio)) return DEFAULT_PREVIEW_RATIO;
  return Math.min(MAX_PREVIEW_RATIO, Math.max(MIN_PREVIEW_RATIO, ratio));
}

/** The saved ratio, or the default. Storage that throws or holds junk is ignored. */
export function loadPreviewRatio(windowId: string): number {
  try {
    const raw = window.localStorage.getItem(previewKey(windowId));
    if (!raw) return DEFAULT_PREVIEW_RATIO;
    const parsed: unknown = JSON.parse(raw);
    if (!parsed || typeof parsed !== "object") return DEFAULT_PREVIEW_RATIO;
    return clampPreviewRatio((parsed as Record<string, unknown>).ratio);
  } catch {
    return DEFAULT_PREVIEW_RATIO;
  }
}

/** Remember the operator's choice. Only a drag end, an arrow key or a double-click calls this; the fit never does. */
export function savePreviewRatio(windowId: string, ratio: number): void {
  try {
    window.localStorage.setItem(previewKey(windowId), JSON.stringify({ ratio: clampPreviewRatio(ratio) }));
  } catch {
    // Storage is full or blocked: the width just is not remembered.
  }
}

export type PreviewFitInput = {
  /** The saved share of the available width the preview takes. */
  ratio: number;
  windowWidth: number;
  /** The workspace row: terminals, preview and run panel. */
  rowWidth: number;
  /** The run panel's width in px, 0 when it is collapsed or absent. */
  runPanelWidth: number;
  /** The row's gap in px; one lies before the preview and, with a run panel, one more before it. */
  gutter: number;
  /** One terminal cell's width in px. */
  cellWidth: number;
  /** The terminal's padding on one side, in px. */
  termPad: number;
  /** The narrowest the preview may be (px). */
  minPreviewPx: number;
  /**
   * A tab split side by side: the narrowest pane's share of the panes' width (0.2..0.5) and the px the split itself
   * takes between and around the panes (its divider and the panes' borders). Absent for one pane, or a split stacked
   * top and bottom, where every terminal is as wide as the whole terminal area.
   */
  split?: { narrowestShare: number; chromePx: number } | null;
};

export type PreviewFit = { mode: "side"; width: number } | { mode: "overlay" };

/** A split's first-pane share is kept within 0.2..0.8 (SplitTerminals' MIN_RATIO/MAX_RATIO and the backend's). */
const SPLIT_MIN_SHARE = 0.2;

/** The narrower pane's share of a split whose first pane has `ratio`: 0.2..0.5, half when the ratio is not a number. */
export function splitNarrowestShare(ratio: number): number {
  if (!Number.isFinite(ratio)) return 0.5;
  return Math.max(SPLIT_MIN_SHARE, Math.min(ratio, 1 - ratio));
}

/**
 * The narrowest the terminal area may be: 60 columns plus padding in its narrowest terminal. In a tab split side by
 * side that is the narrowest pane, so the whole area must be that over the pane's share, plus the split's own chrome.
 */
export function terminalAreaMin(input: Pick<PreviewFitInput, "cellWidth" | "termPad" | "split">): number {
  const one = MIN_TERMINAL_COLS * input.cellWidth + 2 * input.termPad;
  const split = input.split;
  if (!split || !(split.narrowestShare > 0 && split.narrowestShare < 1)) return one;
  return Math.ceil(one / split.narrowestShare + Math.max(0, split.chromePx));
}

/**
 * Where the preview goes. An overlay under 1000 px of window, or when 60 terminal columns (in every pane of a split
 * tab) plus the preview minimum do not fit; else beside the terminals at `ratio` of the width left after the run
 * panel, clamped so every terminal keeps 60 columns. The clamp changes the width shown, never the ratio kept.
 */
export function previewFit(input: PreviewFitInput): PreviewFit {
  const { ratio, windowWidth, minPreviewPx } = input;
  const available = previewAvailable(input);
  const terminalMin = terminalAreaMin(input);
  if (windowWidth < OVERLAY_BELOW_PX || !(available >= terminalMin + minPreviewPx)) return { mode: "overlay" };
  const wanted = Math.round(clampPreviewRatio(ratio) * available);
  const width = Math.floor(Math.min(available - terminalMin, Math.max(minPreviewPx, wanted)));
  return { mode: "side", width };
}

/** The width available to the terminals and the preview, the same arithmetic as `previewFit`. */
export function previewAvailable(input: Pick<PreviewFitInput, "rowWidth" | "runPanelWidth" | "gutter">): number {
  return input.rowWidth - input.runPanelWidth - input.gutter * (input.runPanelWidth > 0 ? 2 : 1);
}
