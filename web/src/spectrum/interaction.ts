// Pointer interaction and hover cursor on an overlay canvas above the spectrum and waterfall.
// Wheel zooms the view (or tunes the selected VFO over its passband / in "wheel tunes" mode),
// plain, shift or middle drag pans a zoomed view, double-click resets the zoom.
// Plain click: moves the selected VFO there, or tunes the device centre when none is selected.
// Click on a VFO selects it, dragging a VFO retunes it, dragging an edge of the selected VFO's
// passband changes its bandwidth, Alt/Ctrl-click creates a VFO. The frequency axis is a pan handle.
// VFO markers and bandwidth shading are drawn here (canvas, no React per frame).
// Hovering previews the click: a cursor line at the snapped landing frequency, a ghost passband
// of the selected VFO and a label; a click glides the marker (~150 ms) and pulses the target.
// Band plan ranges, channel ticks and bookmark flags sit in a strip along the top (annotations.ts);
// a click on a tunable channel or bookmark tunes the selected VFO instead of a plain click.
// The overlay is plain 2D canvas, so all of this works without WebGL2.
// Everything here is imperative; no React state changes per pointer event.

import {
  hitTarget,
  type AnnotationHit,
  type AnnotationLayer,
  type AnnotationTarget,
} from "./annotations";
import { formatFrequency } from "./freq";
import {
  CANVAS_FONT,
  CANVAS_SANS,
  CANVAS_TEXT_STRONG,
  CHIP_BG,
  CHIP_BORDER,
  CURSOR_COLOR,
  WINDOW_EDGE,
  WINDOW_LABEL,
  WINDOW_SHADE,
} from "./palette";
import {
  GLIDE_MS,
  OUTSIDE_LABEL,
  OUTSIDE_LABEL_SHORT,
  OUTSIDE_WINDOW_HINT,
  PULSE_MS,
  glide,
  isOutside,
  prefersReducedMotion,
  previewLabel,
  type PreviewTarget,
} from "./preview";
import {
  MODE_LABEL,
  formatBandwidth,
  hitTestVfo,
  passbandRange,
  vfoColor,
  type Marker,
} from "../vfo/model";
import {
  CLICK_SLOP_PX,
  VFO_GRAB_PX,
  WheelSteps,
  bandwidthFromEdge,
  exceedsSlop,
  hitPassbandEdge,
  isOverPassband,
  wheelAction,
  type PassbandEdge,
  type WheelMode,
} from "./gestures";
import { fracToHz, spanWidth, type Span, type ViewController } from "./view";

/** A click waits this long for a possible second click (double-click resets the zoom). */
const CLICK_DELAY_MS = 220;
const WHEEL_ZOOM_PER_PIXEL = 0.0015;
/** How long after a click a changed VFO frequency still counts as the click's result. */
const CLICK_RESULT_MS = 600;

export interface InteractionOptions {
  canvas: HTMLCanvasElement;
  /** Left margin (CSS px) of the plot area inside the canvas. */
  leftPad: number;
  view: ViewController;
  /** Plain click without a selected VFO: tune the device centre. */
  onTune: (hz: number) => void;
  onVfoSelect: (id: number | null) => void;
  /** Target frequency of a VFO move (already snapped for clicks, raw while dragging). */
  onVfoMove: (id: number, hz: number) => void;
  /**
   * Where a plain click at `hz` would land: snapped to the step of VFO `vfoId`, or whole Hz when
   * the click would tune the centre (`vfoId` null). Drives the hover preview and the click itself.
   */
  snap: (hz: number, vfoId: number | null) => number;
  onVfoCreate: (hz: number) => void;
  /** Band plan strip and bookmarks; the owner feeds it data and calls `refresh()`. */
  annotations: AnnotationLayer;
  /** A tunable channel or bookmark in the strip was clicked. */
  onAnnotationTune: (target: AnnotationTarget) => void;
  /** Wheel preference ("zoom" or "tune"), read on every wheel event. */
  getWheelMode: () => WheelMode;
  /** Wheel over the selected VFO: tune it by `steps` tuning steps (negative = down). */
  onVfoStep: (id: number, steps: number) => void;
  /** A passband edge of the selected VFO was dragged: the new bandwidth (already snapped). */
  onBandwidth: (id: number, bandwidthHz: number) => void;
  /** Vertical extent (CSS px from the canvas top) of the frequency axis, if known. */
  axis?: () => { top: number; bottom: number } | null;
}

export function wheelDelta(e: Pick<WheelEvent, "deltaY" | "deltaMode">): number {
  // Lines and pages are converted to approximate pixels.
  return e.deltaMode === 1 ? e.deltaY * 33 : e.deltaMode === 2 ? e.deltaY * 400 : e.deltaY;
}

interface Drag {
  id: number;
  startX: number;
  base: Span | null;
  moved: boolean;
  pan: boolean;
  /** Set when the press started on a VFO: id and (centre - pointer) offset in Hz. */
  vfo?: { id: number; offsetHz: number };
  /** Alt/Ctrl press: creates a VFO on release. */
  create?: boolean;
  /** Set when the press started on a passband edge of the selected VFO. */
  edge?: { id: number; side: PassbandEdge; last: number };
  /** The press started on the frequency axis (a pan handle, never grabs a VFO). */
  axis?: boolean;
}

export class SpectrumInteraction {
  private frameSpan: Span | null = null;
  private hoverX: number | null = null;
  private hoverY: number | null = null;
  private raf = 0;
  private dpr = 1;
  private clickTimer: ReturnType<typeof setTimeout> | undefined;
  private markers: readonly Marker[] = [];
  private selectedId: number | null = null;
  private window: Span | null = null;
  private hoverMod = false;
  private titleShown = false;
  /** Marker glides after a click: the drawn frequency runs from `from` to `to` since `t0`. */
  private readonly glides = new Map<number, { from: number; to: number; t0: number }>();
  private readonly lastFreq = new Map<number, number>();
  private clickMove: { id: number; until: number } | null = null;
  /** Fading line at the clicked frequency (absolute, so it follows a centre retune). */
  private pulse: { hz: number; t0: number } | null = null;
  private drag: Drag | null = null;
  private readonly wheelSteps = new WheelSteps();
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

  /** Redraw after the annotation data or settings changed. */
  refresh(): void {
    this.schedule();
  }

  /** True once a span is known (from a frame or the tuning state). */
  hasFrameSpan(): boolean {
    return this.frameSpan !== null;
  }

  /** VFO markers to draw and hit-test; `selectedId` is highlighted. */
  setVfos(markers: readonly Marker[], selectedId: number | null): void {
    const now = performance.now();
    const click = this.clickMove && now < this.clickMove.until ? this.clickMove : null;
    for (const m of markers) {
      const before = this.lastFreq.get(m.id);
      const running = this.glides.get(m.id);
      if (running && running.to !== m.freqHz) this.glides.delete(m.id);
      if (
        click?.id === m.id &&
        before !== undefined &&
        before !== m.freqHz &&
        !prefersReducedMotion()
      ) {
        // Start from where the marker is drawn right now, so quick successive clicks stay smooth.
        const from = running ? glide(running.from, running.to, now - running.t0, false) : before;
        this.glides.set(m.id, { from, to: m.freqHz, t0: now });
        this.clickMove = null;
      }
      this.lastFreq.set(m.id, m.freqHz);
    }
    for (const id of [...this.lastFreq.keys()]) {
      if (!markers.some((m) => m.id === id)) {
        this.lastFreq.delete(id);
        this.glides.delete(id);
      }
    }
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
    const sel = this.markers.find((m) => m.id === this.selectedId);
    const hz = this.hzAt(e.clientX);
    const action = wheelAction({
      ctrlKey: e.ctrlKey,
      setting: this.o.getWheelMode(),
      hasVfo: sel !== undefined,
      overPassband:
        sel !== undefined &&
        hz !== null &&
        !this.onAxis(e.clientY) &&
        isOverPassband(sel, hz, this.slopHz()),
    });
    if (action === "tune" && sel) {
      const steps = this.wheelSteps.add(wheelDelta(e));
      if (steps !== 0) this.o.onVfoStep(sel.id, steps);
      return;
    }
    this.wheelSteps.reset();
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
      if (this.onAxis(e.clientY)) {
        this.drag.axis = true;
      } else if (hz !== null) {
        if (e.altKey || e.ctrlKey) {
          this.drag.create = true;
        } else {
          const onStrip = hitTarget(this.annotationAt(e.clientX, e.clientY)) !== null;
          const sel = onStrip ? undefined : this.markers.find((m) => m.id === this.selectedId);
          const side = sel ? hitPassbandEdge(sel, hz, this.slopHz()) : null;
          if (sel && side) {
            this.drag.edge = { id: sel.id, side, last: sel.bandwidthHz };
          } else {
            const hit = onStrip ? null : hitTestVfo(this.markers, hz, this.slopHz());
            if (hit) {
              this.drag.vfo = { id: hit.id, offsetHz: hit.freqHz - hz };
              if (hit.id !== this.selectedId) this.o.onVfoSelect(hit.id);
            }
          }
        }
      }
    }
    this.o.canvas.setPointerCapture?.(e.pointerId);
  };

  private readonly onMove = (e: PointerEvent): void => {
    const rect = this.o.canvas.getBoundingClientRect();
    this.hoverX = e.clientX - rect.left;
    this.hoverY = e.clientY - rect.top;
    this.hoverMod = e.altKey || e.ctrlKey;
    this.schedule();
    const d = this.drag;
    if (!d) {
      this.updateCursor(e.clientX, e.clientY);
      return;
    }
    if (d.id !== e.pointerId) return;
    this.updateDragCursor(d);
    const dx = e.clientX - d.startX;
    if (exceedsSlop(dx, CLICK_SLOP_PX)) d.moved = true;
    if (d.vfo && d.moved) {
      const hz = this.hzAt(e.clientX, true);
      if (hz !== null) this.o.onVfoMove(d.vfo.id, Math.round(hz + d.vfo.offsetHz));
    }
    if (d.edge && d.moved) {
      const hz = this.hzAt(e.clientX, true);
      const m = this.markers.find((v) => v.id === d.edge?.id);
      if (hz !== null && m) {
        const bw = bandwidthFromEdge(m.mode, hz, m.freqHz);
        if (bw !== d.edge.last) {
          d.edge.last = bw;
          this.o.onBandwidth(m.id, bw);
        }
      }
    }
    // Plain drags on empty spectrum or the axis pan like shift/middle drags (no effect unzoomed).
    if (d.moved && !d.pan && !d.vfo && !d.edge && !d.create) d.pan = true;
    if (d.pan && d.moved && d.base) {
      const base = d.base;
      const plotW = rect.width - this.o.leftPad;
      // Pixels map to the span on screen; the requested span keeps its own width.
      const onScreen = this.frameSpan ?? base;
      if (plotW > 0) this.o.view.panFrom(base, -(dx / plotW) * spanWidth(onScreen));
    }
  };

  private readonly onUp = (e: PointerEvent): void => {
    const d = this.drag;
    if (!d || d.id !== e.pointerId) return;
    this.drag = null;
    this.o.canvas.releasePointerCapture?.(e.pointerId);
    this.updateCursor(e.clientX, e.clientY);
    if (d.pan || d.moved || d.vfo || d.edge) return;
    const hz = this.hzAt(e.clientX);
    if (hz === null) return;
    const annotation = hitTarget(this.annotationAt(e.clientX, e.clientY));
    if (annotation) {
      clearTimeout(this.clickTimer);
      this.o.onAnnotationTune(annotation);
      return;
    }
    if (d.create) {
      this.o.onVfoCreate(Math.round(hz));
      return;
    }
    // Wait briefly: a double-click must reset the zoom without retuning twice.
    clearTimeout(this.clickTimer);
    const selected = this.selectedId;
    this.clickTimer = setTimeout(() => {
      const target = this.o.snap(hz, selected);
      const now = performance.now();
      if (!prefersReducedMotion()) this.pulse = { hz: target, t0: now };
      if (selected !== null) {
        this.clickMove = { id: selected, until: now + CLICK_RESULT_MS };
        this.o.onVfoMove(selected, target);
      } else {
        this.o.onTune(target);
      }
      this.schedule();
    }, CLICK_DELAY_MS);
  };

  private readonly onCancel = (): void => {
    this.drag = null;
  };

  private readonly onLeave = (): void => {
    this.hoverX = null;
    this.hoverY = null;
    this.setTitle(false);
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

  /** The strip entry under a pointer position (client coordinates), if any. */
  private annotationAt(clientX: number, clientY: number): AnnotationHit | null {
    const rect = this.o.canvas.getBoundingClientRect();
    const x = clientX - rect.left - this.o.leftPad;
    if (x < 0 || x > rect.width - this.o.leftPad) return null;
    return this.o.annotations.hit(x, clientY - rect.top);
  }

  /** True when a client Y lies on the frequency axis. */
  private onAxis(clientY: number): boolean {
    const band = this.o.axis?.();
    if (!band) return false;
    const y = clientY - this.o.canvas.getBoundingClientRect().top;
    return y >= band.top && y <= band.bottom;
  }

  private updateCursor(clientX: number, clientY: number): void {
    const hz = this.hzAt(clientX);
    const tunable = hitTarget(this.annotationAt(clientX, clientY)) !== null;
    const axis = this.onAxis(clientY);
    const sel = this.markers.find((m) => m.id === this.selectedId);
    const slop = this.slopHz();
    const edge = !tunable && !axis && sel && hz !== null && hitPassbandEdge(sel, hz, slop) !== null;
    const over = !tunable && !axis && hz !== null && hitTestVfo(this.markers, hz, slop) !== null;
    this.o.canvas.style.cursor = tunable
      ? "pointer"
      : edge
        ? "ew-resize"
        : over || (axis && this.o.view.zoomed)
          ? "grab"
          : "";
    this.setTitle(hz !== null && isOutside(this.window, hz));
  }

  /** While a drag is running: grabbing for pans and VFO moves, resize for an edge. */
  private updateDragCursor(d: Drag): void {
    if (!d.moved) return;
    this.o.canvas.style.cursor = d.edge ? "ew-resize" : d.pan || d.vfo ? "grabbing" : "";
  }

  /** Native tooltip explaining the shaded part of the span; only touched when it changes. */
  private setTitle(show: boolean): void {
    if (show === this.titleShown) return;
    this.titleShown = show;
    if (show) this.o.canvas.title = OUTSIDE_WINDOW_HINT;
    else this.o.canvas.removeAttribute("title");
  }

  /** Frequency a marker is drawn at (mid-glide after a click, otherwise its own). */
  private displayFreq(m: Marker, now: number): number {
    const g = this.glides.get(m.id);
    if (!g) return m.freqHz;
    return glide(g.from, g.to, now - g.t0, prefersReducedMotion());
  }

  /** What a click at the pointer would do and where; null target = plain cursor (dragging). */
  private previewAt(rawHz: number): { hz: number; target: PreviewTarget | null } {
    const d = this.drag;
    if (d && (d.pan || d.moved || d.vfo || d.edge)) return { hz: rawHz, target: null };
    const hit = hitTestVfo(this.markers, rawHz, this.slopHz());
    if (hit) return { hz: hit.freqHz, target: { kind: "grab", id: hit.id } };
    if (this.hoverMod) return { hz: Math.round(rawHz), target: { kind: "create" } };
    const sel = this.markers.find((m) => m.id === this.selectedId);
    if (sel) return { hz: this.o.snap(rawHz, sel.id), target: { kind: "vfo", id: sel.id } };
    return { hz: this.o.snap(rawHz, null), target: { kind: "centre" } };
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
    const now = performance.now();
    ctx.clearRect(0, 0, canvas.width, canvas.height);
    const span = this.frameSpan;
    if (!span) return;
    const plotCssW = canvas.width / this.dpr - this.o.leftPad;
    this.o.annotations.update(ctx, span, plotCssW, this.dpr);
    this.drawWindow(ctx, span);
    this.o.annotations.draw(ctx, this.o.leftPad, this.dpr, canvas.height);
    this.drawVfos(ctx, span, now);
    this.drawPulse(ctx, span, now);
    this.drawHover(ctx, span);
    // Keep drawing while a glide or the click pulse is running.
    this.pruneAnimations(now);
    if (this.glides.size > 0 || this.pulse) this.schedule();
  }

  private pruneAnimations(now: number): void {
    for (const [id, g] of this.glides) {
      if (now - g.t0 >= GLIDE_MS) this.glides.delete(id);
    }
    if (this.pulse && now - this.pulse.t0 >= PULSE_MS) this.pulse = null;
  }

  /** Cursor line at the (snapped) landing frequency, ghost passband and the floating label. */
  private drawHover(ctx: CanvasRenderingContext2D, span: Span): void {
    const x = this.hoverX;
    if (x === null) return;
    const canvas = this.o.canvas;
    const dpr = this.dpr;
    const left = this.o.leftPad;
    const plotCssW = canvas.width / dpr - left;
    const frac = (x - left) / plotCssW;
    if (frac < 0 || frac > 1) return;

    const plotLeft = left * dpr;
    const plotW = canvas.width - plotLeft;
    const width = spanWidth(span);
    const xOf = (hz: number) => plotLeft + ((hz - span.startHz) / width) * plotW;
    const { hz, target } = this.previewAt(fracToHz(span, frac));
    const px = target?.kind === "grab" || target === null ? x * dpr : xOf(hz);

    ctx.save();
    ctx.beginPath();
    ctx.rect(plotLeft, 0, plotW, canvas.height);
    ctx.clip();
    if (target?.kind === "vfo") {
      // Ghost of the passband the VFO would get at the landing frequency.
      const m = this.markers.find((v) => v.id === target.id);
      if (m) {
        const range = passbandRange(m.mode, hz, m.bandwidthHz);
        const x0 = xOf(range.loHz);
        const x1 = xOf(range.hiHz);
        const color = vfoColor(m.id);
        ctx.globalAlpha = 0.12;
        ctx.fillStyle = color;
        ctx.fillRect(x0, 0, Math.max(1, x1 - x0), canvas.height);
        ctx.globalAlpha = 0.55;
        ctx.strokeStyle = color;
        ctx.lineWidth = Math.max(1, dpr);
        ctx.setLineDash([4 * dpr, 3 * dpr]);
        ctx.beginPath();
        ctx.moveTo(x0, 0);
        ctx.lineTo(x0, canvas.height);
        ctx.moveTo(x1, 0);
        ctx.lineTo(x1, canvas.height);
        ctx.stroke();
        ctx.setLineDash([]);
        ctx.globalAlpha = 1;
      }
    }
    ctx.strokeStyle = CURSOR_COLOR;
    ctx.lineWidth = Math.max(1, dpr);
    ctx.beginPath();
    ctx.moveTo(px, 0);
    ctx.lineTo(px, canvas.height);
    ctx.stroke();
    ctx.restore();

    const ay = this.hoverY;
    const stripHit = ay !== null && !this.drag?.moved ? this.o.annotations.hit(x - left, ay) : null;
    if (stripHit) {
      this.o.annotations.drawTooltip(ctx, stripHit, left + stripHit.x, canvas.width, dpr);
      return;
    }
    const dragged = this.drag?.edge;
    const edgeMarker = dragged ? this.markers.find((m) => m.id === dragged.id) : undefined;
    const label = edgeMarker
      ? `Bandwidth ${formatBandwidth(edgeMarker.bandwidthHz)}`
      : target
        ? previewLabel(hz, target, isOutside(this.window, hz))
        : `${formatFrequency(hz)} MHz`;
    ctx.font = `${11 * dpr}px ${CANVAS_FONT}`;
    const padX = 4 * dpr;
    const w = ctx.measureText(label).width + padX * 2;
    const h = 16 * dpr;
    const boxX = Math.min(Math.max(plotLeft, px + 6 * dpr), canvas.width - w);
    const labelY = (2 + this.o.annotations.inset) * dpr;
    this.chip(ctx, boxX, labelY, w, h, CHIP_BG);
    ctx.fillStyle = CANVAS_TEXT_STRONG;
    ctx.textBaseline = "middle";
    ctx.textAlign = "left";
    ctx.fillText(label, boxX + padX, labelY + h / 2);
  }

  /** Brief fading line where the last click landed; it keeps its frequency across a retune. */
  private drawPulse(ctx: CanvasRenderingContext2D, span: Span, now: number): void {
    const p = this.pulse;
    if (!p) return;
    const t = (now - p.t0) / PULSE_MS;
    if (t >= 1) return;
    const canvas = this.o.canvas;
    const left = this.o.leftPad * this.dpr;
    const plotW = canvas.width - left;
    const width = spanWidth(span);
    if (plotW <= 0 || width <= 0 || p.hz < span.startHz || p.hz > span.endHz) return;
    const x = left + ((p.hz - span.startHz) / width) * plotW;
    ctx.save();
    ctx.globalAlpha = 0.8 * (1 - t);
    ctx.strokeStyle = CANVAS_TEXT_STRONG;
    ctx.lineWidth = Math.max(1, this.dpr) * (1 + 2 * (1 - t));
    ctx.beginPath();
    ctx.moveTo(x, 0);
    ctx.lineTo(x, canvas.height);
    ctx.stroke();
    ctx.restore();
  }

  /**
   * Shade what lies outside the usable window, mark its edges with a dashed line and label the
   * shaded sides at the top edge (the hover tooltip explains the rule).
   */
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
    ctx.setLineDash([]);
    ctx.font = `${10 * dpr}px ${CANVAS_SANS}`;
    ctx.fillStyle = WINDOW_LABEL;
    ctx.textBaseline = "top";
    const gap = 6 * dpr;
    const text = (room: number) =>
      [OUTSIDE_LABEL, OUTSIDE_LABEL_SHORT].find((t) => ctx.measureText(t).width + gap * 2 <= room);
    const leftText = x0 > left ? text(x0 - left) : undefined;
    if (leftText) {
      ctx.textAlign = "right";
      ctx.fillText(leftText, x0 - gap, (4 + this.o.annotations.inset) * dpr);
    }
    const rightText = x1 < right ? text(right - x1) : undefined;
    if (rightText) {
      ctx.textAlign = "left";
      ctx.fillText(rightText, x1 + gap, (4 + this.o.annotations.inset) * dpr);
    }
    ctx.restore();
  }

  private drawVfos(ctx: CanvasRenderingContext2D, span: Span, now: number): void {
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
      const freq = this.displayFreq(m, now);
      const range = passbandRange(m.mode, freq, m.bandwidthHz);
      const x0 = xOf(range.loHz);
      const x1 = xOf(range.hiHz);
      const xc = xOf(freq);
      if (Math.max(x1, xc) < left || Math.min(x0, xc) > canvas.width) continue;
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
      const chipY = (22 + this.o.annotations.inset) * dpr;
      this.chip(ctx, bx, chipY, w, h, CHIP_BG, color);
      ctx.fillStyle = color;
      ctx.fillText(label, bx + padX, chipY + h / 2);
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
