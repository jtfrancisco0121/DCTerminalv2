/** Dragging a tab to a new place in the tab bar. */

/** A drag starts after the pointer moved this many pixels (less is a click). */
export const DRAG_THRESHOLD_PX = 5;

/**
 * Where the dragged tab lands: the number of the other tabs whose middle is
 * left of the pointer. `centres` are the other tabs' middles, in order.
 * Moving past a tab's midpoint moves the dragged tab past that tab.
 */
export function dropIndex(centres: readonly number[], pointerX: number): number {
  return centres.filter((centre) => centre < pointerX).length;
}

/** The x of the insert marker: before the other tab at `index`, or after the last. */
export function markerX(
  others: readonly { left: number; right: number }[],
  index: number,
): number | null {
  if (others.length === 0) return null;
  if (index >= others.length) return others[others.length - 1].right;
  return others[index].left;
}
