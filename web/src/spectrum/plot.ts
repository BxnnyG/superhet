// 2D canvas spectrum line plot with dB grid and a frequency axis in MHz.

import type { SpectrumFrame } from "./frame";
import type { DisplayRange } from "./levels";
import {
  CANVAS_AXIS_BG,
  CANVAS_BG,
  CANVAS_FONT,
  CANVAS_GRID,
  CANVAS_TEXT,
  TRACE_FILL,
  TRACE_STROKE,
  WINDOW_EDGE,
  WINDOW_LABEL,
} from "./palette";
import type { Span } from "./view";

export const AXIS_HEIGHT = 18;
/** Left margin (CSS px) reserved for the dB labels; the waterfall is offset by the same amount. */
export const PLOT_LEFT_PAD = 34;

/** Pick a "nice" tick step (1, 2, 5 × 10^n) giving roughly `target` ticks over `span`. */
export function niceStep(span: number, target: number): number {
  const raw = span / Math.max(1, target);
  const pow = 10 ** Math.floor(Math.log10(raw));
  const f = raw / pow;
  return (f < 1.5 ? 1 : f < 3.5 ? 2 : f < 7.5 ? 5 : 10) * pow;
}

export function formatMHz(hz: number, step: number): string {
  const decimals = Math.min(6, Math.max(0, Math.ceil(-Math.log10(step / 1e6))));
  return (hz / 1e6).toFixed(decimals);
}

export class SpectrumPlot {
  private readonly ctx: CanvasRenderingContext2D;
  private frame: SpectrumFrame | null = null;
  private window: Span | null = null;
  private raf = 0;
  private dpr = 1;
  private destroyed = false;

  /** `range` is read on every draw; mutate it and call `redraw()` to change the y-axis. */
  constructor(
    private readonly canvas: HTMLCanvasElement,
    private readonly range: DisplayRange,
  ) {
    const ctx = canvas.getContext("2d");
    if (!ctx) throw new Error("2D canvas is not available");
    this.ctx = ctx;
  }

  /** Set the backing store size in device pixels; `dpr` scales fonts and line widths. */
  resize(width: number, height: number, dpr: number): void {
    this.dpr = dpr;
    this.canvas.width = Math.max(1, Math.floor(width));
    this.canvas.height = Math.max(1, Math.floor(height));
    this.schedule();
  }

  /** Redraw the last frame with the current display range. */
  redraw(): void {
    this.schedule();
  }

  /** Usable window of the device; its edges are flagged on the frequency axis. */
  setWindow(window: Span | null): void {
    this.window = window;
    this.schedule();
  }

  push(frame: SpectrumFrame): void {
    this.frame = frame;
    this.schedule();
  }

  destroy(): void {
    this.destroyed = true;
    cancelAnimationFrame(this.raf);
  }

  private schedule(): void {
    if (this.raf || this.destroyed) return;
    this.raf = requestAnimationFrame(() => {
      this.raf = 0;
      this.draw();
    });
  }

  private draw(): void {
    const { ctx, canvas, dpr } = this;
    const w = canvas.width;
    const h = canvas.height;
    ctx.fillStyle = CANVAS_BG;
    ctx.fillRect(0, 0, w, h);
    const frame = this.frame;
    if (!frame || frame.binCount === 0) return;

    const left = PLOT_LEFT_PAD * dpr;
    const axisH = AXIS_HEIGHT * dpr;
    const plotW = w - left;
    const plotH = h - axisH;
    if (plotW <= 0 || plotH <= 0) return;

    ctx.fillStyle = CANVAS_AXIS_BG;
    ctx.fillRect(0, plotH, w, axisH);
    ctx.font = `${10 * dpr}px ${CANVAS_FONT}`;
    ctx.lineWidth = Math.max(1, dpr * 0.5);

    // dB grid.
    const { lowDb, highDb } = this.range;
    const dbRange = highDb - lowDb;
    if (!(dbRange > 0)) return;
    const dbStep = niceStep(dbRange, Math.max(2, plotH / (40 * dpr)));
    ctx.textAlign = "right";
    ctx.textBaseline = "middle";
    for (let db = Math.ceil(lowDb / dbStep) * dbStep; db <= highDb; db += dbStep) {
      const y = plotH - ((db - lowDb) / dbRange) * plotH;
      ctx.strokeStyle = CANVAS_GRID;
      ctx.beginPath();
      ctx.moveTo(left, y);
      ctx.lineTo(w, y);
      ctx.stroke();
      ctx.fillStyle = CANVAS_TEXT;
      ctx.fillText(String(Math.round(db)), left - 4 * dpr, y);
    }

    // Frequency grid and labels.
    const startHz = frame.centerHz - frame.sampleRate / 2;
    const fStep = niceStep(frame.sampleRate, plotW / (90 * dpr));
    ctx.textAlign = "center";
    ctx.textBaseline = "top";
    for (let f = Math.ceil(startHz / fStep) * fStep; f <= startHz + frame.sampleRate; f += fStep) {
      const x = left + ((f - startHz) / frame.sampleRate) * plotW;
      ctx.strokeStyle = CANVAS_GRID;
      ctx.beginPath();
      ctx.moveTo(x, 0);
      ctx.lineTo(x, plotH);
      ctx.stroke();
      ctx.fillStyle = CANVAS_TEXT;
      ctx.fillText(formatMHz(f, fStep), x, plotH + 3 * dpr);
    }

    // Usable-window edges on the axis: a tick plus the exact frequency, on the inner side so the
    // label stays clear of the shaded area drawn by the overlay.
    const win = this.window;
    if (win) {
      const edges = [
        { hz: win.startHz, align: "left" as const },
        { hz: win.endHz, align: "right" as const },
      ];
      ctx.font = `${10 * dpr}px ${CANVAS_FONT}`;
      ctx.textBaseline = "top";
      for (const { hz, align } of edges) {
        if (hz < startHz || hz > startHz + frame.sampleRate) continue;
        const x = left + ((hz - startHz) / frame.sampleRate) * plotW;
        const text = (hz / 1e6).toFixed(3);
        const tw = ctx.measureText(text).width;
        const pad = 3 * dpr;
        ctx.fillStyle = CANVAS_AXIS_BG;
        ctx.fillRect(align === "left" ? x : x - tw - pad * 2, plotH, tw + pad * 2, axisH);
        ctx.strokeStyle = WINDOW_EDGE;
        ctx.lineWidth = Math.max(1, dpr);
        ctx.beginPath();
        ctx.moveTo(x, plotH);
        ctx.lineTo(x, h);
        ctx.stroke();
        ctx.fillStyle = WINDOW_LABEL;
        ctx.textAlign = align;
        ctx.fillText(text, align === "left" ? x + pad : x - pad, plotH + 3 * dpr);
      }
    }

    // Trace.
    const n = frame.binCount;
    const binDb = (frame.maxDb - frame.minDb) / 255;
    const path = new Path2D();
    for (let i = 0; i < n; i++) {
      const x = left + ((i + 0.5) / n) * plotW;
      const db = frame.minDb + (frame.bins[i] ?? 0) * binDb;
      const y = plotH - Math.min(1, Math.max(0, (db - lowDb) / dbRange)) * plotH;
      if (i === 0) path.moveTo(x, y);
      else path.lineTo(x, y);
    }
    const fill = new Path2D(path);
    fill.lineTo(left + ((n - 0.5) / n) * plotW, plotH);
    fill.lineTo(left + (0.5 / n) * plotW, plotH);
    fill.closePath();
    ctx.fillStyle = TRACE_FILL;
    ctx.fill(fill);
    ctx.strokeStyle = TRACE_STROKE;
    ctx.lineWidth = Math.max(1, dpr);
    ctx.stroke(path);
  }
}
