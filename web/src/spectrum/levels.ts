// Display level (contrast) handling: the dB window mapped onto the colormap and plot y-axis,
// plus an auto-level estimator. Pure logic, no DOM; the renderers read `DisplayRange`.

/** dB window shown by the plot and waterfall. Mutated in place; renderers read it per draw. */
export interface DisplayRange {
  lowDb: number;
  highDb: number;
}

/** Smallest allowed window width in dB. */
export const MIN_SPAN_DB = 5;

const BUCKETS = 256;
const NOISE_PERCENTILE = 0.2;
const PEAK_PERCENTILE = 0.995;
const FLOOR_MARGIN_DB = 5;
const MIN_RANGE_ABOVE_FLOOR_DB = 40;
const PEAK_MARGIN_DB = 5;
/** EMA time constant for floor and peak estimates. */
const TAU_SECONDS = 1;

/** Frames further apart than this (e.g. after a pause) are treated as one tick of this length. */
const MAX_DT_SECONDS = 1;

export interface LevelFrame {
  bins: Uint8Array;
  /** dBFS mapped to bin value 0. */
  minDb: number;
  /** dBFS mapped to bin value 255. */
  maxDb: number;
}

/** Smallest bucket index whose cumulative count reaches `fraction` of `total`. */
function percentileBucket(hist: Uint32Array, total: number, fraction: number): number {
  const target = Math.max(1, Math.ceil(fraction * total));
  let cum = 0;
  for (let i = 0; i < BUCKETS; i++) {
    cum += hist[i] ?? 0;
    if (cum >= target) return i;
  }
  return BUCKETS - 1;
}

/**
 * Tracks the noise floor and strong-peak level of the incoming frames and derives a display
 * range from them. Uses a preallocated 256-bucket histogram, so no per-frame allocation.
 */
export class LevelEstimator {
  private readonly hist = new Uint32Array(BUCKETS);
  private floorDb = NaN;
  private peakDb = NaN;

  reset(): void {
    this.floorDb = NaN;
    this.peakDb = NaN;
  }

  /** Feed one frame; `dtSeconds` is the time since the previous frame. Writes into `out`. */
  update(frame: LevelFrame, dtSeconds: number, out: DisplayRange): void {
    const n = frame.bins.length;
    if (n === 0) return;
    const hist = this.hist;
    hist.fill(0);
    for (let i = 0; i < n; i++) {
      const b = frame.bins[i] ?? 0;
      hist[b] = (hist[b] ?? 0) + 1;
    }
    const scale = (frame.maxDb - frame.minDb) / (BUCKETS - 1);
    const floor = frame.minDb + percentileBucket(hist, n, NOISE_PERCENTILE) * scale;
    const peak = frame.minDb + percentileBucket(hist, n, PEAK_PERCENTILE) * scale;

    if (Number.isNaN(this.floorDb) || Number.isNaN(this.peakDb)) {
      this.floorDb = floor;
      this.peakDb = peak;
    } else {
      const dt = Math.min(Math.max(dtSeconds, 0), MAX_DT_SECONDS);
      const alpha = 1 - Math.exp(-dt / TAU_SECONDS);
      this.floorDb += alpha * (floor - this.floorDb);
      this.peakDb += alpha * (peak - this.peakDb);
    }

    let low = Math.max(frame.minDb, this.floorDb - FLOOR_MARGIN_DB);
    let high = Math.min(
      frame.maxDb,
      Math.max(this.floorDb + MIN_RANGE_ABOVE_FLOOR_DB, this.peakDb + PEAK_MARGIN_DB),
    );
    if (high - low < MIN_SPAN_DB) {
      // Only possible when clamped against the frame's own limits.
      if (low + MIN_SPAN_DB <= frame.maxDb) high = low + MIN_SPAN_DB;
      else {
        high = frame.maxDb;
        low = high - MIN_SPAN_DB;
      }
    }
    out.lowDb = low;
    out.highDb = high;
  }
}

export interface LevelSettings {
  auto: boolean;
  manualLowDb: number;
  manualHighDb: number;
}

export const DEFAULT_LEVEL_SETTINGS: LevelSettings = {
  auto: true,
  manualLowDb: -100,
  manualHighDb: -20,
};

const STORAGE_KEY = "superhet.levels.v1";

function defaultStorage(): Pick<Storage, "getItem" | "setItem"> | null {
  try {
    return globalThis.localStorage ?? null;
  } catch {
    return null; // access can throw when storage is blocked
  }
}

export function loadLevelSettings(
  storage: Pick<Storage, "getItem"> | null = defaultStorage(),
): LevelSettings {
  try {
    const raw = storage?.getItem(STORAGE_KEY);
    if (!raw) return { ...DEFAULT_LEVEL_SETTINGS };
    const v: unknown = JSON.parse(raw);
    if (typeof v !== "object" || v === null) return { ...DEFAULT_LEVEL_SETTINGS };
    const o = v as Record<string, unknown>;
    const low = o.manualLowDb;
    const high = o.manualHighDb;
    if (
      typeof o.auto !== "boolean" ||
      typeof low !== "number" ||
      typeof high !== "number" ||
      !Number.isFinite(low) ||
      !Number.isFinite(high) ||
      high - low < MIN_SPAN_DB
    ) {
      return { ...DEFAULT_LEVEL_SETTINGS };
    }
    return { auto: o.auto, manualLowDb: low, manualHighDb: high };
  } catch {
    return { ...DEFAULT_LEVEL_SETTINGS };
  }
}

export function saveLevelSettings(
  settings: LevelSettings,
  storage: Pick<Storage, "setItem"> | null = defaultStorage(),
): void {
  try {
    storage?.setItem(STORAGE_KEY, JSON.stringify(settings));
  } catch {
    // Persistence is best effort (quota, private mode).
  }
}

/**
 * Owns the display range shared by the renderers. In auto mode `onFrame` updates the range
 * from each frame; in manual mode the range stays at the manual values. Changes made through
 * the setters notify listeners so that renderers can redraw without waiting for a frame.
 */
export class LevelController {
  readonly range: DisplayRange;
  auto: boolean;
  private readonly estimator = new LevelEstimator();
  private readonly listeners = new Set<() => void>();
  private lastMs = NaN;

  constructor(settings: LevelSettings = DEFAULT_LEVEL_SETTINGS) {
    this.auto = settings.auto;
    this.range = { lowDb: settings.manualLowDb, highDb: settings.manualHighDb };
  }

  settings(): LevelSettings {
    return { auto: this.auto, manualLowDb: this.range.lowDb, manualHighDb: this.range.highDb };
  }

  /** Call for every frame before it is handed to the renderers. */
  onFrame(frame: LevelFrame, nowMs: number): void {
    if (!this.auto) return;
    const dt = Number.isNaN(this.lastMs) ? 0 : (nowMs - this.lastMs) / 1000;
    this.lastMs = nowMs;
    this.estimator.update(frame, dt, this.range);
  }

  setAuto(): void {
    this.auto = true;
    this.estimator.reset();
    this.lastMs = NaN;
    this.notify();
  }

  /** Switch to manual mode with the given window (swapped/widened to honour MIN_SPAN_DB). */
  setManual(lowDb: number, highDb: number): void {
    this.auto = false;
    this.range.lowDb = lowDb;
    this.range.highDb = Math.max(highDb, lowDb + MIN_SPAN_DB);
    this.notify();
  }

  onChange(listener: () => void): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  private notify(): void {
    for (const l of this.listeners) l();
  }
}
