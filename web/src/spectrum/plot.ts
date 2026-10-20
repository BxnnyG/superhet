// 2D canvas spectrum line plot with dB grid and a frequency axis in MHz.

import type { SpectrumFrame } from "./frame";

const AXIS_HEIGHT = 18;
const LEFT_PAD = 34;

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
  private raf = 0;
  private dpr = 1;
  private destroyed = false;

  constructor(private readonly canvas: HTMLCanvasElement) {
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
    ctx.fillStyle = "#0a0a0c";
    ctx.fillRect(0, 0, w, h);
    const frame = this.frame;
    if (!frame || frame.binCount === 0) return;

    const left = LEFT_PAD * dpr;
    const axisH = AXIS_HEIGHT * dpr;
    const plotW = w - left;
    const plotH = h - axisH;
    if (plotW <= 0 || plotH <= 0) return;

    ctx.font = `${10 * dpr}px ui-monospace, monospace`;
    ctx.lineWidth = Math.max(1, dpr * 0.5);

    // dB grid.
    const dbRange = frame.maxDb - frame.minDb;
    const dbStep = niceStep(dbRange, Math.max(2, plotH / (40 * dpr)));
    ctx.textAlign = "right";
    ctx.textBaseline = "middle";
    for (let db = Math.ceil(frame.minDb / dbStep) * dbStep; db <= frame.maxDb; db += dbStep) {
      const y = plotH - ((db - frame.minDb) / dbRange) * plotH;
      ctx.strokeStyle = "#26262c";
      ctx.beginPath();
      ctx.moveTo(left, y);
      ctx.lineTo(w, y);
      ctx.stroke();
      ctx.fillStyle = "#8a8a94";
      ctx.fillText(String(Math.round(db)), left - 4 * dpr, y);
    }

    // Frequency grid and labels.
    const startHz = frame.centerHz - frame.sampleRate / 2;
    const fStep = niceStep(frame.sampleRate, plotW / (90 * dpr));
    ctx.textAlign = "center";
    ctx.textBaseline = "top";
    for (let f = Math.ceil(startHz / fStep) * fStep; f <= startHz + frame.sampleRate; f += fStep) {
      const x = left + ((f - startHz) / frame.sampleRate) * plotW;
      ctx.strokeStyle = "#26262c";
      ctx.beginPath();
      ctx.moveTo(x, 0);
      ctx.lineTo(x, plotH);
      ctx.stroke();
      ctx.fillStyle = "#8a8a94";
      ctx.fillText(formatMHz(f, fStep), x, plotH + 3 * dpr);
    }

    // Trace.
    const n = frame.binCount;
    const path = new Path2D();
    for (let i = 0; i < n; i++) {
      const x = left + ((i + 0.5) / n) * plotW;
      const y = plotH - ((frame.bins[i] ?? 0) / 255) * plotH;
      if (i === 0) path.moveTo(x, y);
      else path.lineTo(x, y);
    }
    const fill = new Path2D(path);
    fill.lineTo(left + ((n - 0.5) / n) * plotW, plotH);
    fill.lineTo(left + (0.5 / n) * plotW, plotH);
    fill.closePath();
    ctx.fillStyle = "rgba(56, 189, 248, 0.15)";
    ctx.fill(fill);
    ctx.strokeStyle = "#38bdf8";
    ctx.lineWidth = Math.max(1, dpr);
    ctx.stroke(path);
  }
}
