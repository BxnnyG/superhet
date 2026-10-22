import { describe, expect, it } from "vitest";
import { RowHistory, TABLE_STRIDE, resampleBins, rowHasData, rowMapping } from "./history";

const span = (startHz: number, endHz: number) => ({ startHz, endHz });

describe("rowMapping", () => {
  it("is the identity for the same span", () => {
    const m = rowMapping(span(100e6, 102e6), span(100e6, 102e6));
    expect(m.scale).toBe(1);
    expect(m.offset).toBe(0);
  });

  it("zooming in shows a part of an older, wider row", () => {
    // View is the centre half of the row: x = 0 -> u = 0.25, x = 1 -> u = 0.75.
    const m = rowMapping(span(100e6, 104e6), span(101e6, 103e6));
    expect(m.scale).toBeCloseTo(0.5);
    expect(m.offset).toBeCloseTo(0.25);
    expect(rowHasData(m, 0)).toBe(true);
    expect(rowHasData(m, 1)).toBe(true);
  });

  it("zooming out leaves the part beyond a narrow row empty", () => {
    // Row covers the middle quarter of the view.
    const m = rowMapping(span(101.5e6, 102.5e6), span(100e6, 104e6));
    expect(m.scale).toBeCloseTo(4);
    expect(rowHasData(m, 0.1)).toBe(false);
    expect(rowHasData(m, 0.5)).toBe(true);
    expect(rowHasData(m, 0.9)).toBe(false);
    // The row's own start/end land at the right screen positions.
    expect(0.375 * m.scale + m.offset).toBeCloseTo(0);
    expect(0.625 * m.scale + m.offset).toBeCloseTo(1);
  });

  it("panning shifts the row: the same frequency keeps its meaning", () => {
    const row = span(100e6, 102e6);
    const view = span(100.5e6, 102.5e6);
    const m = rowMapping(row, view);
    // 101 MHz is at x = 0.25 of the view and at u = 0.5 of the row.
    expect(0.25 * m.scale + m.offset).toBeCloseTo(0.5);
    expect(rowHasData(m, 0.9)).toBe(false);
  });

  it("maps rows across a centre retune with full Hz precision at VHF", () => {
    const row = span(145_000_000 - 1_200_000, 145_000_000 + 1_200_000);
    const view = span(145_900_000 - 1_200_000, 145_900_000 + 1_200_000);
    const m = rowMapping(row, view);
    const x = (145.5e6 - view.startHz) / 2.4e6;
    expect(x * m.scale + m.offset).toBeCloseTo((145.5e6 - row.startHz) / 2.4e6, 9);
  });

  it("cannot map degenerate spans", () => {
    expect(rowMapping(span(1, 1), span(0, 10)).scale).toBe(0);
    expect(rowMapping(span(0, 10), span(5, 5)).scale).toBe(0);
    expect(rowHasData({ scale: 0, offset: 0 }, 0.5)).toBe(false);
  });
});

describe("RowHistory", () => {
  it("reports unwritten rows as unmappable and written rows relative to the view", () => {
    const h = new RowHistory(4);
    h.set(1, span(100e6, 102e6), -100, -20);
    const out = new Float32Array(4 * TABLE_STRIDE);
    h.fillTable(out, span(100e6, 101e6));
    expect(out[0]).toBe(0);
    expect(out[TABLE_STRIDE]).toBeCloseTo(0.5);
    expect(out[TABLE_STRIDE + 1]).toBeCloseTo(0);
    expect(out[TABLE_STRIDE + 2]).toBe(-100);
    expect(out[TABLE_STRIDE + 3]).toBe(-20);
    h.clear();
    h.fillTable(out, span(100e6, 101e6));
    expect(out[TABLE_STRIDE]).toBe(0);
  });

  it("keeps rows of different spans side by side", () => {
    const h = new RowHistory(2);
    h.set(0, span(100e6, 102e6), -100, -20);
    h.set(1, span(100.5e6, 101.5e6), -90, -10);
    const out = new Float32Array(2 * TABLE_STRIDE);
    h.fillTable(out, span(100e6, 102e6));
    expect(out[0]).toBeCloseTo(1);
    expect(out[TABLE_STRIDE]).toBeCloseTo(2);
    expect(out[TABLE_STRIDE + 1]).toBeCloseTo(-0.5);
  });
});

describe("resampleBins", () => {
  it("copies equal widths", () => {
    const dst = new Uint8Array(3);
    resampleBins(Uint8Array.of(1, 2, 3), dst);
    expect([...dst]).toEqual([1, 2, 3]);
  });

  it("interpolates when upsampling and keeps the ends", () => {
    const dst = new Uint8Array(8);
    resampleBins(Uint8Array.of(0, 100), dst);
    expect(dst[0]).toBe(0);
    expect(dst[7]).toBe(100);
    for (let i = 1; i < 8; i++) expect(dst[i]).toBeGreaterThanOrEqual(dst[i - 1] ?? 0);
  });

  it("keeps peaks when downsampling", () => {
    const src = new Uint8Array(8);
    src[5] = 200;
    const dst = new Uint8Array(4);
    resampleBins(src, dst);
    expect([...dst]).toEqual([0, 0, 200, 0]);
  });
});
