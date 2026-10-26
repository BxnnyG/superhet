// Windowing for the bookmark list: rows have fixed heights per kind, so the visible slice is a
// binary search over their cumulative offsets. Pure; the panel owns scroll and size state.

export interface RowLayout {
  /** Top edge of each row in px. */
  offsets: number[];
  total: number;
}

export function layoutRows(heights: readonly number[]): RowLayout {
  const offsets: number[] = [];
  let y = 0;
  for (const h of heights) {
    offsets.push(y);
    y += h;
  }
  return { offsets, total: y };
}

/**
 * Rows `[start, end)` to render for a viewport of `viewportH` px scrolled by `scrollTop`, with
 * `overscan` extra rows on each side.
 */
export function visibleWindow(
  layout: RowLayout,
  heights: readonly number[],
  scrollTop: number,
  viewportH: number,
  overscan = 4,
): { start: number; end: number } {
  const n = heights.length;
  if (n === 0) return { start: 0, end: 0 };
  const bottom = scrollTop + viewportH;
  // First row whose bottom edge is below the top of the viewport.
  let lo = 0;
  let hi = n;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if ((layout.offsets[mid] ?? 0) + (heights[mid] ?? 0) <= scrollTop) lo = mid + 1;
    else hi = mid;
  }
  let end = lo;
  while (end < n && (layout.offsets[end] ?? 0) < bottom) end++;
  return { start: Math.max(0, lo - overscan), end: Math.min(n, end + overscan) };
}
