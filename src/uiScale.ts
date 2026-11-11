/**
 * UI scale: one percent applied to the root font size, so every rem length in
 * App.css (text, spacing, sizes) follows it. The terminal font is never scaled:
 * xterm has its own size setting and zoom keys. This module is the only place
 * that sets the root font size; it caches the choice so the next start paints
 * at the right scale before settings load, and announces a change so components
 * that measure px (xterm fit, the tab drag) can measure again.
 */

/** The offered scales. The same list as `UI_SCALES` in settings_store.rs (a test keeps them equal). */
export const UI_SCALES: readonly number[] = [90, 100, 110, 125];
export const DEFAULT_UI_SCALE = 100;
/** Fired on `window` after the root font size changes; `detail` is the percent. */
export const UI_SCALE_EVENT = "dcv2:ui-scale";

const CACHE_KEY = "dcterminalv2.uiScale";

/** A saved value as a scale: unknown, missing or junk values become 100. */
export function normalizeUiScale(value: unknown): number {
  const n = typeof value === "string" ? Number(value) : value;
  return typeof n === "number" && UI_SCALES.includes(n) ? n : DEFAULT_UI_SCALE;
}

export function cachedUiScale(): number {
  try {
    return normalizeUiScale(window.localStorage.getItem(CACHE_KEY));
  } catch {
    return DEFAULT_UI_SCALE;
  }
}

/** Set the root font size to `percent` (an unlisted value becomes 100), cache it and announce it. Returns the scale applied. */
export function applyUiScale(percent: unknown): number {
  const scale = normalizeUiScale(percent);
  document.documentElement.style.fontSize = `${scale}%`;
  try {
    window.localStorage.setItem(CACHE_KEY, String(scale));
  } catch {
    // Private mode or storage disabled: the setting still lives in settings.json.
  }
  window.dispatchEvent(new CustomEvent<number>(UI_SCALE_EVENT, { detail: scale }));
  return scale;
}
