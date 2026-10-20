// Pointer interaction and hover cursor on an overlay canvas above the spectrum and waterfall.
// Wheel zooms the view, shift/middle drag pans, click tunes, double-click resets the zoom.
// Everything here is imperative; no React state changes per pointer event.

import { formatFrequency } from "./freq";
import { fracToHz, spanWidth, type Span, type ViewController } from "./view";

/** Distance in CSS pixels below which a press counts as a click. */
const CLICK_SLOP_PX = 4;
/** A click waits this long for a possible second click (double-click resets the zoom). */
const CLICK_DELAY_MS = 220;
const WHEEL_ZOOM_PER_PIXEL = 0.0015;

export interface InteractionOptions {
  canvas: HTMLCanvasElement;
  /** Left margin (CSS px) of the plot area inside the canvas. */
  leftPad: number;
  view: ViewController;
  onTune: (hz: number) => void;
}

export function wheelDelta(e: Pick<WheelEvent, "deltaY" | "deltaMode">): number {
  // Lines and pages are converted to approximate pixels.
  return e.deltaMode === 1 ? e.deltaY * 33 : e.deltaMode === 2 ? e.deltaY * 400 : e.deltaY;
}

export class SpectrumInteraction {
  private frameSpan: Span | null = null;
  private hoverX: number | null = null;
  private raf = 0;
  private dpr = 1;
  private clickTimer: ReturnType<typeof setTimeout> | undefined;
  private drag: {
    id: number;
    startX: number;
    base: Span | null;
    moved: boolean;
    pan: boolean;
  } | null = null;
  private readonly ctx: CanvasRenderingContext2D | null;
  private destroyed = false;

  constructor(private readonly o: InteractionOptions) {
    this.ctx = o.canvas.getContext("2d");
    const c = o.canvas;
    c.addEventListener("wheel", this.onWheel, { passive: false });
    c.addEventListener("pointerdown", this.onDown);
    c.addEventListener("pointermove", this.onMove);
    c.addEventListener("pointerup", this.onUp);
    c.addEventListener("pointercancel", this.onCancel);
    c.addEventListener("pointerleave", this.onLeave);
    c.addEventListener("dblclick", this.onDouble);
    c.addEventListener("auxclick", this.onAux);
  }

  destroy(): void {
    this.destroyed = true;
    const c = this.o.canvas;
    c.removeEventListener("wheel", this.onWheel);
    c.removeEventListener("pointerdown", this.onDown);
    c.removeEventListener("pointermove", this.onMove);
    c.removeEventListener("pointerup", this.onUp);
    c.removeEventListener("pointercancel", this.onCancel);
    c.removeEventListener("pointerleave", this.onLeave);
    c.removeEventListener("dblclick", this.onDouble);
    c.removeEventListener("auxclick", this.onAux);
    clearTimeout(this.clickTimer);
    cancelAnimationFrame(this.raf);
  }

  resize(width: number, height: number, dpr: number): void {
    this.dpr = dpr;
    this.o.canvas.width = Math.max(1, Math.floor(width));
    this.o.canvas.height = Math.max(1, Math.floor(height));
    this.schedule();
  }

  /** Span covered by the frame on screen; used to map the pointer to a frequency. */
  setFrameSpan(centerHz: number, sampleRate: number): void {
    const s = this.frameSpan;
    const start = centerHz - sampleRate / 2;
    if (s && s.startHz === start && spanWidth(s) === sampleRate) return;
    this.frameSpan = { startHz: start, endHz: start + sampleRate };
    if (this.hoverX !== null) this.schedule();
  }

  /** Fraction (0..1) across the plot area for a client X, or null outside it. */
  private fracAt(clientX: number): number | null {
    const rect = this.o.canvas.getBoundingClientRect();
    const plotW = rect.width - this.o.leftPad;
    if (plotW <= 0) return null;
    const f = (clientX - rect.left - this.o.leftPad) / plotW;
    return f < 0 || f > 1 ? null : f;
  }

  private readonly onWheel = (e: WheelEvent): void => {
    const frac = this.fracAt(e.clientX);
    if (frac === null) return;
    e.preventDefault();
    this.o.view.zoom(frac, Math.exp(wheelDelta(e) * WHEEL_ZOOM_PER_PIXEL));
  };

  private readonly onDown = (e: PointerEvent): void => {
    const pan = e.button === 1 || (e.button === 0 && e.shiftKey);
    if (e.button !== 0 && e.button !== 1) return;
    const base = this.o.view.current();
    if (pan) e.preventDefault();
    this.drag = { id: e.pointerId, startX: e.clientX, base, moved: false, pan };
    this.o.canvas.setPointerCapture?.(e.pointerId);
  };

  private readonly onMove = (e: PointerEvent): void => {
    const rect = this.o.canvas.getBoundingClientRect();
    this.hoverX = e.clientX - rect.left;
    this.schedule();
    const d = this.drag;
    if (!d || d.id !== e.pointerId) return;
    const dx = e.clientX - d.startX;
    if (Math.abs(dx) > CLICK_SLOP_PX) d.moved = true;
    if (d.pan && d.moved && d.base) {
      const base = d.base;
      const plotW = rect.width - this.o.leftPad;
      if (plotW > 0) this.o.view.panFrom(base, -(dx / plotW) * spanWidth(base));
    }
  };

  private readonly onUp = (e: PointerEvent): void => {
    const d = this.drag;
    if (!d || d.id !== e.pointerId) return;
    this.drag = null;
    this.o.canvas.releasePointerCapture?.(e.pointerId);
    if (d.pan || d.moved) return;
    const hz = this.hzAt(e.clientX);
    if (hz === null) return;
    // Wait briefly: a double-click must reset the zoom without retuning twice.
    clearTimeout(this.clickTimer);
    this.clickTimer = setTimeout(() => this.o.onTune(Math.round(hz)), CLICK_DELAY_MS);
  };

  private readonly onCancel = (): void => {
    this.drag = null;
  };

  private readonly onLeave = (): void => {
    this.hoverX = null;
    this.schedule();
  };

  private readonly onDouble = (): void => {
    clearTimeout(this.clickTimer);
    this.o.view.reset();
  };

  private readonly onAux = (e: MouseEvent): void => {
    if (e.button === 1) e.preventDefault();
  };

  private hzAt(clientX: number): number | null {
    const frac = this.fracAt(clientX);
    const span = this.frameSpan;
    return frac === null || !span ? null : fracToHz(span, frac);
  }

  private schedule(): void {
    if (this.raf || this.destroyed) return;
    this.raf = requestAnimationFrame(() => {
      this.raf = 0;
      this.draw();
    });
  }

  private draw(): void {
    const { ctx } = this;
    const canvas = this.o.canvas;
    if (!ctx) return;
    ctx.clearRect(0, 0, canvas.width, canvas.height);
    const span = this.frameSpan;
    const x = this.hoverX;
    if (x === null || !span) return;
    const dpr = this.dpr;
    const left = this.o.leftPad;
    const plotCssW = canvas.width / dpr - left;
    const frac = (x - left) / plotCssW;
    if (frac < 0 || frac > 1) return;

    const px = x * dpr;
    ctx.strokeStyle = "rgba(250, 204, 21, 0.7)";
    ctx.lineWidth = Math.max(1, dpr);
    ctx.beginPath();
    ctx.moveTo(px, 0);
    ctx.lineTo(px, canvas.height);
    ctx.stroke();

    const label = `${formatFrequency(fracToHz(span, frac))} MHz`;
    ctx.font = `${11 * dpr}px ui-monospace, monospace`;
    const padX = 4 * dpr;
    const w = ctx.measureText(label).width + padX * 2;
    const h = 16 * dpr;
    const boxX = Math.min(Math.max(left * dpr, px + 6 * dpr), canvas.width - w);
    ctx.fillStyle = "rgba(10, 10, 12, 0.85)";
    ctx.fillRect(boxX, 2 * dpr, w, h);
    ctx.fillStyle = "#facc15";
    ctx.textBaseline = "middle";
    ctx.textAlign = "left";
    ctx.fillText(label, boxX + padX, 2 * dpr + h / 2);
  }
}
