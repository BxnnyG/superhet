// Waterfall history bookkeeping. Every row remembers the absolute frequency span (and dB mapping)
// it was rendered with, so a change of zoom, pan or centre only changes how old rows are mapped
// onto the screen, never whether they exist. Pure functions plus a small typed-array store; the
// WebGL side uploads `fillTable` output as a texture.

import { spanWidth, type Span } from "./view";

/** Floats per row in the table handed to the shader: scale, offset, minDb, maxDb. */
export const TABLE_STRIDE = 4;

/**
 * Map a screen position x (0..1 across the view) to a position u (0..1 across the row's own span):
 * `u = x * scale + offset`. `scale` is 0 when the row cannot be mapped.
 */
export function rowMapping(row: Span, view: Span): { scale: number; offset: number } {
  const rw = spanWidth(row);
  const vw = spanWidth(view);
  if (!(rw > 0) || !(vw > 0)) return { scale: 0, offset: 0 };
  return { scale: vw / rw, offset: (view.startHz - row.startHz) / rw };
}

/** True when screen position `x` falls inside the row's span (otherwise: background). */
export function rowHasData(mapping: { scale: number; offset: number }, x: number): boolean {
  if (!(mapping.scale > 0)) return false;
  const u = x * mapping.scale + mapping.offset;
  return u >= 0 && u <= 1;
}

/**
 * Resample `src` onto `dst.length` texels with the bin-centre convention (bin i covers
 * [i / n, (i + 1) / n]). Fewer bins than texels interpolate linearly; more bins keep the maximum
 * of the bins a texel covers so narrow peaks survive.
 */
export function resampleBins(src: Uint8Array, dst: Uint8Array): void {
  const n = src.length;
  const w = dst.length;
  if (n === 0 || w === 0) return;
  if (n === w) {
    dst.set(src);
    return;
  }
  if (n > w) {
    const ratio = n / w;
    for (let j = 0; j < w; j++) {
      const from = Math.floor(j * ratio);
      const to = Math.max(from + 1, Math.floor((j + 1) * ratio));
      let max = 0;
      for (let i = from; i < to && i < n; i++) max = Math.max(max, src[i] ?? 0);
      dst[j] = max;
    }
    return;
  }
  const ratio = n / w;
  for (let j = 0; j < w; j++) {
    const pos = Math.min(n - 1, Math.max(0, (j + 0.5) * ratio - 0.5));
    const i0 = Math.floor(pos);
    const i1 = Math.min(n - 1, i0 + 1);
    const f = pos - i0;
    dst[j] = Math.round((src[i0] ?? 0) * (1 - f) + (src[i1] ?? 0) * f);
  }
}

/** Per ring slot: the span the row was rendered with and the dB range of its byte values. */
export class RowHistory {
  private readonly start: Float64Array;
  private readonly width: Float64Array;
  private readonly minDb: Float32Array;
  private readonly maxDb: Float32Array;

  constructor(readonly rows: number) {
    this.start = new Float64Array(rows);
    this.width = new Float64Array(rows);
    this.minDb = new Float32Array(rows);
    this.maxDb = new Float32Array(rows);
  }

  set(slot: number, span: Span, minDb: number, maxDb: number): void {
    this.start[slot] = span.startHz;
    this.width[slot] = spanWidth(span);
    this.minDb[slot] = minDb;
    this.maxDb[slot] = maxDb;
  }

  clear(): void {
    this.start.fill(0);
    this.width.fill(0);
  }

  spanOf(slot: number): Span | null {
    const w = this.width[slot] ?? 0;
    return w > 0 ? { startHz: this.start[slot] ?? 0, endHz: (this.start[slot] ?? 0) + w } : null;
  }

  /**
   * Fill `out` (rows * TABLE_STRIDE floats) with the mapping of every row onto `view`. Computed in
   * doubles here so the shader only deals with small, well-conditioned numbers.
   */
  fillTable(out: Float32Array, view: Span): void {
    for (let i = 0; i < this.rows; i++) {
      const span = this.spanOf(i);
      const m = span ? rowMapping(span, view) : { scale: 0, offset: 0 };
      const o = i * TABLE_STRIDE;
      out[o] = m.scale;
      out[o + 1] = m.offset;
      out[o + 2] = this.minDb[i] ?? 0;
      out[o + 3] = this.maxDb[i] ?? 0;
    }
  }
}
