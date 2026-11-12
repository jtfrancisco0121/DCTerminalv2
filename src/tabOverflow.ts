/** A tab's horizontal box and the visible span of the tab row, in the same coordinates (client px). */
export type TabBox = { id: string; left: number; right: number };
export type TabView = { left: number; right: number };

/**
 * The ids of the tabs that are not fully inside the visible span of the row, in tab order. A tab cut on either
 * edge counts as hidden: the "+N" button and its menu both list exactly this array.
 */
export function hiddenTabs(boxes: ReadonlyArray<TabBox>, view: TabView): string[] {
  return boxes.filter((box) => box.left < view.left || box.right > view.right).map((box) => box.id);
}
