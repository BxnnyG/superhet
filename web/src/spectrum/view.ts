// Zoom/pan math for the displayed span. Pure functions plus a small throttled controller that
// forwards the requested span to the WebSocket without flooding it.

export interface Span {
  startHz: number;
  endHz: number;
}

/** Narrowest span the user can zoom to. */
export const MIN_VIEW_SPAN_HZ = 10_000;

export function spanWidth(s: Span): number {
  return s.endHz - s.startHz;
}

export function spanCenter(s: Span): number {
  return (s.startHz + s.endHz) / 2;
}

export function deviceSpan(centerHz: number, sampleRate: number): Span {
  return { startHz: centerHz - sampleRate / 2, endHz: centerHz + sampleRate / 2 };
}

/** Limit the width to [min(minSpan, device), device] and shift the span into `device`. */
export function clampSpan(view: Span, device: Span, minSpan = MIN_VIEW_SPAN_HZ): Span {
  const dw = spanWidth(device);
  const width = Math.min(dw, Math.max(Math.min(minSpan, dw), spanWidth(view)));
  const start = Math.min(device.endHz - width, Math.max(device.startHz, view.startHz));
  return { startHz: start, endHz: start + width };
}

/** True when `view` covers (almost) the whole device span. */
export function isFullSpan(view: Span, device: Span): boolean {
  return spanWidth(view) >= spanWidth(device) * 0.999;
}

/**
 * Scale the span width by `factor` (<1 zooms in) keeping the frequency at `anchorFrac`
 * (0..1 across the screen) under the same screen position.
 *
 * `shown` is the span the screen currently displays. It can differ from `view`, the span asked
 * for last: frames lag the request, and the daemon snaps the edges outwards to its bins. The
 * pointer fraction belongs to what is on screen, so the anchor frequency comes from `shown`
 * while the width keeps scaling the requested `view` (successive wheel steps accumulate).
 */
export function zoomSpan(
  view: Span,
  device: Span,
  anchorFrac: number,
  factor: number,
  minSpan = MIN_VIEW_SPAN_HZ,
  shown: Span = view,
): Span {
  const frac = Math.min(1, Math.max(0, anchorFrac));
  const width = spanWidth(view);
  const anchorHz = shown.startHz + frac * spanWidth(shown);
  const nextWidth = Math.min(
    spanWidth(device),
    Math.max(Math.min(minSpan, spanWidth(device)), width * factor),
  );
  const start = anchorHz - frac * nextWidth;
  return clampSpan({ startHz: start, endHz: start + nextWidth }, device, minSpan);
}

/** Shift the span by `deltaHz`, staying inside the device span. */
export function panSpan(view: Span, device: Span, deltaHz: number): Span {
  return clampSpan(
    { startHz: view.startHz + deltaHz, endHz: view.endHz + deltaHz },
    device,
    spanWidth(view),
  );
}

/** Frequency at `frac` (0..1) across the span. */
export function fracToHz(span: Span, frac: number): number {
  return span.startHz + frac * spanWidth(span);
}

export interface ViewControllerOptions {
  /** Called with the span to show, or null for the full device span. */
  apply: (view: Span | null) => void;
  /** Minimum time between two applies. */
  minIntervalMs?: number;
  now?: () => number;
  schedule?: (cb: () => void) => number;
  cancel?: (id: number) => void;
}

/**
 * Holds the requested view (null = full device span) and forwards changes at most once per
 * animation frame and `minIntervalMs`; the latest request wins.
 */
export class ViewController {
  private device: Span | null = null;
  private view: Span | null = null;
  /** Span of the newest frame, i.e. what the screen maps pointer positions to. */
  private shownSpan: Span | null = null;
  private dirty = false;
  private timer = 0;
  private lastApply = -Infinity;
  private readonly now: () => number;
  private readonly schedule: (cb: () => void) => number;
  private readonly cancel: (id: number) => void;
  private readonly minInterval: number;

  constructor(private readonly opts: ViewControllerOptions) {
    this.now = opts.now ?? (() => performance.now());
    this.schedule = opts.schedule ?? ((cb) => requestAnimationFrame(cb));
    this.cancel = opts.cancel ?? ((id) => cancelAnimationFrame(id));
    this.minInterval = opts.minIntervalMs ?? 40;
  }

  /** The span currently requested (the full device span when not zoomed). */
  current(): Span | null {
    return this.view ?? this.device;
  }

  get zoomed(): boolean {
    return this.view !== null;
  }

  /** Tell the controller which span the screen shows now (the span of the newest frame). */
  setShown(span: Span): void {
    this.shownSpan = span;
  }

  /** The span on screen, or null before the first frame. */
  shown(): Span | null {
    return this.shownSpan;
  }

  /**
   * Update the device span (confirmed tuning). A changed centre recentres a zoomed view on the
   * new centre; otherwise the view is only clamped into the new span.
   */
  setDevice(next: Span): void {
    const prev = this.device;
    this.device = next;
    if (!this.view || !prev) return;
    const recentre = spanCenter(prev) !== spanCenter(next);
    let view = this.view;
    if (recentre) {
      const w = Math.min(spanWidth(view), spanWidth(next));
      view = { startHz: spanCenter(next) - w / 2, endHz: spanCenter(next) + w / 2 };
    }
    this.assign(clampSpan(view, next));
  }

  zoom(anchorFrac: number, factor: number): void {
    const device = this.device;
    const cur = this.current();
    if (!device || !cur) return;
    this.assign(zoomSpan(cur, device, anchorFrac, factor, MIN_VIEW_SPAN_HZ, this.shownSpan ?? cur));
  }

  /** Pan relative to `base` (the span when the drag started). No effect when not zoomed. */
  panFrom(base: Span, deltaHz: number): void {
    if (!this.device || !this.view) return;
    this.assign(panSpan(base, this.device, deltaHz));
  }

  reset(): void {
    if (!this.view) return;
    this.view = null;
    this.markDirty();
  }

  dispose(): void {
    this.cancel(this.timer);
    this.timer = 0;
    this.dirty = false;
  }

  private assign(next: Span): void {
    const device = this.device;
    this.view = device && isFullSpan(next, device) ? null : next;
    this.markDirty();
  }

  private markDirty(): void {
    this.dirty = true;
    if (this.timer) return;
    this.timer = this.schedule(() => this.flush());
  }

  private flush(): void {
    this.timer = 0;
    if (!this.dirty) return;
    if (this.now() - this.lastApply < this.minInterval) {
      this.timer = this.schedule(() => this.flush());
      return;
    }
    this.dirty = false;
    this.lastApply = this.now();
    const v = this.view;
    this.opts.apply(v ? { startHz: Math.round(v.startHz), endHz: Math.round(v.endHz) } : null);
  }
}
