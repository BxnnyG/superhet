// Pointer interaction and hover cursor on an overlay canvas above the spectrum and waterfall.
// Wheel zooms the view, shift/middle drag pans, double-click resets the zoom.
// Plain click: moves the selected VFO there, or tunes the device centre when none is selected.
// Click on a VFO selects it, dragging a VFO retunes it, Alt/Ctrl-click creates a VFO.
// VFO markers and bandwidth shading are drawn here (canvas, no React per frame).
// Everything here is imperative; no React state changes per pointer event.

import { formatFrequency } from "./freq";
import {
  CANVAS_FONT,
  CANVAS_TEXT_STRONG,
  CHIP_BG,
  CHIP_BORDER,
  CURSOR_COLOR,
  WINDOW_EDGE,
  WINDOW_SHADE,
} from "./palette";
import { MODE_LABEL, hitTestVfo, vfoColor, type Marker } from "../vfo/model";
import { fracToHz, spanWidth, type Span, type ViewController } from "./view";

/** Distance in CSS pixels below which a press counts as a click. */
const CLICK_SLOP_PX = 4;
/** A click waits this long for a possible second click (double-click resets the zoom). */
const CLICK_DELAY_MS = 220;
const WHEEL_ZOOM_PER_PIXEL = 0.0015;
/** Grab tolerance around a VFO centre line (CSS px). */
const VFO_GRAB_PX = 5;

export interface InteractionOptions {
  canvas: HTMLCanvasElement;
  /** Left margin (CSS px) of the plot area inside the canvas. */
  leftPad: number;
  view: ViewController;
  /** Plain click without a selected VFO: tune the device centre. */
  onTune: (hz: number) => void;
  onVfoSelect: (id: number | null) => void;
  /** Raw (unsnapped) frequency; also used for click-to-move of the selected VFO. */
  onVfoMove: (id: number, hz: number) => void;
  onVfoCreate: (hz: number) => void;
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
  private markers: readonly Marker[] = [];
  private selectedId: number | null = null;
  private window: Span | null = null;
  private drag: {
    id: number;
    startX: number;
    base: Span | null;
    moved: boolean;
    pan: boolean;
    /** Set when the press started on a VFO: id and (centre - pointer) offset in Hz. */
    vfo?: { id: number; offsetHz: number };
    /** Alt/Ctrl press: creates a VFO on release. */
    create?: boolean;
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

  /**
   * Span covered by the frame on screen; maps the pointer and the VFO/window overlays to
   * frequencies. Every change redraws, so the overlays never depend on the waterfall or on the
   * pointer hovering.
   */
  setFrameSpan(centerHz: number, sampleRate: number): void {
    const s = this.frameSpan;
    const start = centerHz - sampleRate / 2;
    if (s && s.startHz === start && spanWidth(s) === sampleRate) return;
    this.frameSpan = { startHz: start, endHz: start + sampleRate };
    this.schedule();
  }

  /** True once a span is known (from a frame or the tuning state). */
  hasFrameSpan(): boolean {
    return this.frameSpan !== null;
  }

  /** VFO markers to draw and hit-test; `selectedId` is highlighted. */
  setVfos(markers: readonly Marker[], selectedId: number | null): void {
    this.markers = markers;
    this.selectedId = selectedId;
    this.schedule();
  }

  /** Usable window of the device; the parts of the span outside it are shaded. */
  setWindow(window: Span | null): void {
    const w = this.window;
    if (w === window || (w && window && w.startHz === window.startHz && w.endHz === window.endHz)) {
      return;
    }
    this.window = window;
    this.schedule();
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
    if (!pan && e.button === 0) {
      const hz = this.hzAt(e.clientX);
      if (hz !== null) {
        if (e.altKey || e.ctrlKey) {
          this.drag.create = true;
        } else {
          const hit = hitTestVfo(this.markers, hz, this.slopHz());
          if (hit) {
            this.drag.vfo = { id: hit.id, offsetHz: hit.freqHz - hz };
            if (hit.id !== this.selectedId) this.o.onVfoSelect(hit.id);
          }
        }
      }
    }
    this.o.canvas.setPointerCapture?.(e.pointerId);
  };

  private readonly onMove = (e: PointerEvent): void => {
    const rect = this.o.canvas.getBoundingClientRect();
    this.hoverX = e.clientX - rect.left;
    this.schedule();
    const d = this.drag;
    if (!d) {
      this.updateCursor(e.clientX);
      return;
    }
    if (d.id !== e.pointerId) return;
    const dx = e.clientX - d.startX;
    if (Math.abs(dx) > CLICK_SLOP_PX) d.moved = true;
    if (d.vfo && d.moved) {
      const hz = this.hzAt(e.clientX, true);
      if (hz !== null) this.o.onVfoMove(d.vfo.id, Math.round(hz + d.vfo.offsetHz));
    }
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
    if (d.pan || d.moved || d.vfo) return;
    const hz = this.hzAt(e.clientX);
    if (hz === null) return;
    if (d.create) {
      this.o.onVfoCreate(Math.round(hz));
      return;
    }
    // Wait briefly: a double-click must reset the zoom without retuning twice.
    clearTimeout(this.clickTimer);
    const selected = this.selectedId;
    this.clickTimer = setTimeout(() => {
      if (selected !== null) this.o.onVfoMove(selected, Math.round(hz));
      else this.o.onTune(Math.round(hz));
    }, CLICK_DELAY_MS);
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

  private hzAt(clientX: number, clamp = false): number | null {
    let frac = this.fracAt(clientX);
    const span = this.frameSpan;
    if (!span) return null;
    if (frac === null) {
      if (!clamp) return null;
      const rect = this.o.canvas.getBoundingClientRect();
      frac = clientX < rect.left + this.o.leftPad ? 0 : 1;
    }
    return fracToHz(span, frac);
  }

  /** Grab tolerance in Hz at the current zoom. */
  private slopHz(): number {
    const span = this.frameSpan;
    const plotW = this.o.canvas.getBoundingClientRect().width - this.o.leftPad;
    return span && plotW > 0 ? (spanWidth(span) / plotW) * VFO_GRAB_PX : 0;
  }

  private updateCursor(clientX: number): void {
    const hz = this.hzAt(clientX);
    const over = hz !== null && hitTestVfo(this.markers, hz, this.slopHz()) !== null;
    this.o.canvas.style.cursor = over ? "ew-resize" : "";
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
    if (!span) return;
    this.drawWindow(ctx, span);
    this.drawVfos(ctx, span);
    const x = this.hoverX;
    if (x === null) return;
    const dpr = this.dpr;
    const left = this.o.leftPad;
    const plotCssW = canvas.width / dpr - left;
    const frac = (x - left) / plotCssW;
    if (frac < 0 || frac > 1) return;

    const px = x * dpr;
    ctx.strokeStyle = CURSOR_COLOR;
    ctx.lineWidth = Math.max(1, dpr);
    ctx.beginPath();
    ctx.moveTo(px, 0);
    ctx.lineTo(px, canvas.height);
    ctx.stroke();

    const label = `${formatFrequency(fracToHz(span, frac))} MHz`;
    ctx.font = `${11 * dpr}px ${CANVAS_FONT}`;
    const padX = 4 * dpr;
    const w = ctx.measureText(label).width + padX * 2;
    const h = 16 * dpr;
    const boxX = Math.min(Math.max(left * dpr, px + 6 * dpr), canvas.width - w);
    this.chip(ctx, boxX, 2 * dpr, w, h, CHIP_BG);
    ctx.fillStyle = CANVAS_TEXT_STRONG;
    ctx.textBaseline = "middle";
    ctx.textAlign = "left";
    ctx.fillText(label, boxX + padX, 2 * dpr + h / 2);
  }

  /** Shade what lies outside the usable window and mark its edges with a dashed line. */
  private drawWindow(ctx: CanvasRenderingContext2D, span: Span): void {
    const win = this.window;
    if (!win) return;
    const canvas = this.o.canvas;
    const dpr = this.dpr;
    const left = this.o.leftPad * dpr;
    const plotW = canvas.width - left;
    const width = spanWidth(span);
    if (plotW <= 0 || width <= 0) return;
    const xOf = (hz: number) => left + ((hz - span.startHz) / width) * plotW;
    const right = left + plotW;
    const x0 = Math.min(right, Math.max(left, xOf(win.startHz)));
    const x1 = Math.min(right, Math.max(left, xOf(win.endHz)));
    ctx.save();
    ctx.fillStyle = WINDOW_SHADE;
    if (x0 > left) ctx.fillRect(left, 0, x0 - left, canvas.height);
    if (x1 < right) ctx.fillRect(x1, 0, right - x1, canvas.height);
    ctx.strokeStyle = WINDOW_EDGE;
    ctx.lineWidth = Math.max(1, dpr);
    ctx.setLineDash([3 * dpr, 4 * dpr]);
    for (const [hz, x] of [
      [win.startHz, x0],
      [win.endHz, x1],
    ] as const) {
      if (hz < span.startHz || hz > span.endHz) continue;
      ctx.beginPath();
      ctx.moveTo(x, 0);
      ctx.lineTo(x, canvas.height);
      ctx.stroke();
    }
    ctx.restore();
  }

  private drawVfos(ctx: CanvasRenderingContext2D, span: Span): void {
    if (this.markers.length === 0) return;
    const canvas = this.o.canvas;
    const dpr = this.dpr;
    const left = this.o.leftPad * dpr;
    const plotW = canvas.width - left;
    if (plotW <= 0) return;
    const width = spanWidth(span);
    const xOf = (hz: number) => left + ((hz - span.startHz) / width) * plotW;
    ctx.save();
    ctx.beginPath();
    ctx.rect(left, 0, plotW, canvas.height);
    ctx.clip();
    ctx.font = `${11 * dpr}px ${CANVAS_FONT}`;
    ctx.textBaseline = "middle";
    ctx.textAlign = "left";
    for (const m of this.markers) {
      const selected = m.id === this.selectedId;
      const color = vfoColor(m.id);
      const x0 = xOf(m.freqHz - m.bandwidthHz / 2);
      const x1 = xOf(m.freqHz + m.bandwidthHz / 2);
      const xc = xOf(m.freqHz);
      if (x1 < left || x0 > canvas.width) continue;
      ctx.globalAlpha = m.paused ? 0.35 : 1;
      ctx.fillStyle = color;
      ctx.globalAlpha *= selected ? 0.28 : 0.15;
      ctx.fillRect(x0, 0, Math.max(1, x1 - x0), canvas.height);
      ctx.globalAlpha = m.paused ? 0.5 : 1;
      ctx.strokeStyle = color;
      ctx.lineWidth = Math.max(1, dpr) * (selected ? 2 : 1);
      ctx.setLineDash(m.paused ? [4 * dpr, 4 * dpr] : []);
      ctx.beginPath();
      ctx.moveTo(xc, 0);
      ctx.lineTo(xc, canvas.height);
      ctx.stroke();
      ctx.setLineDash([]);
      const label = `${m.id} ${MODE_LABEL[m.mode]}${m.paused ? " (paused)" : ""}${
        selected ? " " + formatFrequency(m.freqHz) : ""
      }`;
      const padX = 4 * dpr;
      const w = ctx.measureText(label).width + padX * 2;
      const h = 16 * dpr;
      const bx = Math.min(Math.max(left, xc + 3 * dpr), canvas.width - w);
      ctx.globalAlpha = 0.9;
      this.chip(ctx, bx, 22 * dpr, w, h, CHIP_BG, color);
      ctx.fillStyle = color;
      ctx.fillText(label, bx + padX, 22 * dpr + h / 2);
    }
    ctx.restore();
  }

  /** Rounded label chip with a subtle border (outlined in `edge` when given). */
  private chip(
    ctx: CanvasRenderingContext2D,
    x: number,
    y: number,
    w: number,
    h: number,
    fill: string,
    edge?: string,
  ): void {
    const r = 4 * this.dpr;
    ctx.beginPath();
    if (typeof ctx.roundRect === "function") ctx.roundRect(x, y, w, h, r);
    else ctx.rect(x, y, w, h);
    ctx.fillStyle = fill;
    ctx.fill();
    ctx.lineWidth = Math.max(1, this.dpr);
    ctx.strokeStyle = edge ?? CHIP_BORDER;
    ctx.stroke();
  }
}
