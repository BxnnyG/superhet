import { describe, expect, it } from "vitest";
import { layoutRows, visibleWindow } from "./virtual";

describe("visibleWindow", () => {
  const heights = [28, 44, 44, 44, 28, 44, 44, 44, 44, 44];
  const layout = layoutRows(heights);

  it("lays rows out one after another", () => {
    expect(layout.offsets).toEqual([0, 28, 72, 116, 160, 188, 232, 276, 320, 364]);
    expect(layout.total).toBe(408);
  });

  it("returns everything for a tall viewport and nothing for no rows", () => {
    expect(visibleWindow(layout, heights, 0, 1000)).toEqual({ start: 0, end: 10 });
    expect(visibleWindow(layoutRows([]), [], 0, 100)).toEqual({ start: 0, end: 0 });
  });

  it("returns the rows in view plus the overscan", () => {
    // Viewport 200..300 covers rows 4 (160..188 is above), 5, 6.
    expect(visibleWindow(layout, heights, 200, 100, 0)).toEqual({ start: 5, end: 8 });
    expect(visibleWindow(layout, heights, 200, 100, 2)).toEqual({ start: 3, end: 10 });
  });

  it("clamps scrolling past the end", () => {
    const w = visibleWindow(layout, heights, 10_000, 100, 1);
    expect(w.end).toBe(10);
    expect(w.start).toBeLessThanOrEqual(10);
  });
});
