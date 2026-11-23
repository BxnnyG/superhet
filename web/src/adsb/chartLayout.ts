// Geometry of the aircraft panel's charts (altitude, speed, climb rate): which of the recorded
// values fall into the window, where the line goes and how large the value range is.

export interface ChartPoint {
  /** Ms since epoch. */
  t: number;
  /** Null breaks the line (value not reported at that moment). */
  v: number | null;
}

export interface ChartLayout {
  /** SVG path, with gaps where values are missing. */
  path: string;
  min: number;
  max: number;
  /** How much time the line covers, ms. */
  spanMs: number;
  /** The newest value. */
  last: number | null;
  /** Y of the zero line, when zero was asked to stay in view. */
  zeroY: number | null;
}

export const CHART_W = 300;
export const CHART_H = 64;
const PAD_Y = 6;

/** Layout of the chart; null while there are fewer than two values to draw a line through. */
export function chartLayout(
  points: readonly ChartPoint[],
  nowMs: number,
  windowMs: number,
  includeZero: boolean,
): ChartLayout | null {
  const inWindow = points.filter((p) => nowMs - p.t <= windowMs);
  const values = inWindow.flatMap((p) => (p.v === null ? [] : [p.v]));
  if (values.length < 2) return null;
  let min = Math.min(...values);
  let max = Math.max(...values);
  if (includeZero) {
    min = Math.min(min, 0);
    max = Math.max(max, 0);
  }
  if (max - min < 1e-9) {
    // A flat line sits in the middle rather than on the edge.
    min -= 1;
    max += 1;
  }
  const first = inWindow[0]?.t ?? nowMs;
  const spanMs = Math.max(1000, nowMs - first);
  const x = (t: number): number => ((t - first) / spanMs) * CHART_W;
  const y = (v: number): number => PAD_Y + (1 - (v - min) / (max - min)) * (CHART_H - 2 * PAD_Y);
  let path = "";
  let pen = false;
  for (const p of inWindow) {
    if (p.v === null) {
      pen = false;
      continue;
    }
    path += `${pen ? "L" : "M"}${x(p.t).toFixed(1)} ${y(p.v).toFixed(1)}`;
    pen = true;
  }
  const newest = [...inWindow].reverse().find((p) => p.v !== null);
  return {
    path,
    min,
    max,
    spanMs,
    last: newest?.v ?? null,
    zeroY: includeZero ? y(0) : null,
  };
}

/** "45 s" / "12 min". */
export function spanText(ms: number): string {
  const s = Math.round(ms / 1000);
  return s < 90 ? `${s} s` : `${Math.round(s / 60)} min`;
}
