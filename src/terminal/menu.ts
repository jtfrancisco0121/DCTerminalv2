/** Keep a context menu fully inside the window. */
export function clampMenuPosition(
  at: { x: number; y: number },
  size: { width: number; height: number },
  viewport: { width: number; height: number },
  margin = 4,
): { x: number; y: number } {
  const maxX = Math.max(margin, viewport.width - size.width - margin);
  const maxY = Math.max(margin, viewport.height - size.height - margin);
  return {
    x: Math.min(Math.max(at.x, margin), maxX),
    y: Math.min(Math.max(at.y, margin), maxY),
  };
}

/** The next enabled item index, wrapping. `step` is 1 (down) or -1 (up). */
export function nextMenuIndex(current: number, step: 1 | -1, enabled: boolean[]): number {
  const count = enabled.length;
  if (!enabled.some(Boolean)) return -1;
  let index = current;
  for (let tries = 0; tries < count; tries += 1) {
    index = (index + step + count) % count;
    if (enabled[index]) return index;
  }
  return current;
}
