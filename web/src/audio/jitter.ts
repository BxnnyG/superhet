// Pure jitter buffer with clock-drift control, used by the AudioWorklet (one instance per VFO).
// Input: 48 kHz mono PCM tagged with `sample_pos`; output: any hardware rate.
//
// - Playback starts once the fill level reaches the target, an underrun outputs silence and
//   rebuffers, an overrun (fill > 3x target) drops the oldest audio down to the target.
// - Lost frames (gap in sample_pos) are replaced by silence so the stream keeps its timing.
// - Drift control: the read position advances by `nominal * (1 + adj)` per output sample, where
//   `adj` comes from a PI controller on the low-passed fill level, limited to +-0.5 %. Samples
//   are interpolated with a Catmull-Rom cubic, so ratio changes cause no audible steps.
// No allocation in `pull`; plain TS so it can be unit-tested outside the worklet.

export const DEFAULT_TARGET_MS = 100;
/** Maximum playback ratio deviation used for drift control. */
export const MAX_RATIO_ADJ = 0.005;
/** Proportional gain per unit of relative fill error ((fill - target) / target). */
export const KP = 0.04;
/** Integral gain per unit of relative fill error and second. */
export const KI = 0.004;
/** Time constant of the fill-level low-pass in seconds. */
export const FILL_TAU_S = 0.25;
/** Overrun threshold as a multiple of the target fill. */
export const OVERRUN_FACTOR = 3;
/** Gaps longer than this are treated as a discontinuity (no silence inserted). */
export const MAX_GAP_MS = 500;

export type JitterState = "buffering" | "playing";

export interface JitterOptions {
  inputRate?: number;
  outputRate: number;
  targetMs?: number;
  capacityMs?: number;
}

export interface JitterStats {
  state: JitterState;
  /** Buffered audio in ms (input domain). */
  fillMs: number;
  targetMs: number;
  /** Current playback ratio adjustment (0.001 = 0.1 % faster). */
  adj: number;
  underruns: number;
  overruns: number;
  /** Samples replaced by silence because of lost frames. */
  gapSamples: number;
}

export class JitterBuffer {
  readonly inputRate: number;
  readonly outputRate: number;
  private readonly ring: Float32Array;
  private readonly capacity: number;
  private readonly nominalStep: number;
  private target: number;
  /** Absolute count of samples written; the sample with index i lives at ring[i mod capacity]. */
  private writePos = 0;
  /** Absolute (fractional) read position. */
  private readPos = 0;
  private state: JitterState = "buffering";
  private fillAvg = 0;
  private integral = 0;
  private adj = 0;
  private nextPos: number | null = null;
  private readonly zeros = new Float32Array(4800);
  underruns = 0;
  overruns = 0;
  gapSamples = 0;

  constructor(opts: JitterOptions) {
    this.inputRate = opts.inputRate ?? 48_000;
    this.outputRate = opts.outputRate;
    this.nominalStep = this.inputRate / this.outputRate;
    this.capacity = Math.ceil((this.inputRate * (opts.capacityMs ?? 1000)) / 1000);
    this.ring = new Float32Array(this.capacity);
    this.target = 1;
    this.setTargetMs(opts.targetMs ?? DEFAULT_TARGET_MS);
  }

  setTargetMs(ms: number): void {
    const wanted = Math.round((this.inputRate * ms) / 1000);
    const limit = Math.floor((this.capacity - 8) / OVERRUN_FACTOR);
    this.target = Math.max(4, Math.min(wanted, limit));
  }

  /** Buffered input samples. */
  fill(): number {
    return this.writePos - this.readPos;
  }

  stats(): JitterStats {
    return {
      state: this.state,
      fillMs: (this.fill() / this.inputRate) * 1000,
      targetMs: (this.target / this.inputRate) * 1000,
      adj: this.adj,
      underruns: this.underruns,
      overruns: this.overruns,
      gapSamples: this.gapSamples,
    };
  }

  /**
   * Append decoded samples. `samplePos` (48 kHz counter of the first sample) lets the buffer
   * replace lost frames by silence and drop duplicates; omit it for gapless input.
   */
  push(samples: Float32Array, samplePos?: number): void {
    let data = samples;
    if (samplePos !== undefined) {
      if (this.nextPos === null) this.nextPos = samplePos;
      const gap = samplePos - this.nextPos;
      if (gap > 0) {
        if (gap <= (this.inputRate * MAX_GAP_MS) / 1000) {
          this.writeSilence(gap);
          this.gapSamples += gap;
        }
      } else if (gap < 0) {
        // Late or duplicate data: skip what has already been accounted for.
        if (-gap >= data.length) return;
        data = data.subarray(-gap);
      }
      this.nextPos = samplePos + samples.length;
    }
    this.write(data);
    if (this.state === "playing" && this.fill() > this.target * OVERRUN_FACTOR) {
      this.readPos = this.writePos - this.target;
      this.fillAvg = this.target;
      this.overruns += 1;
    } else if (this.fill() > this.capacity - 8) {
      // Not playing (e.g. a very long prefill): keep the newest data only.
      this.readPos = this.writePos - this.target;
    }
  }

  private write(data: Float32Array): void {
    const src = data.length > this.capacity ? data.subarray(data.length - this.capacity) : data;
    const n = src.length;
    const start = this.writePos % this.capacity;
    const first = Math.min(n, this.capacity - start);
    this.ring.set(src.subarray(0, first), start);
    if (first < n) this.ring.set(src.subarray(first), 0);
    this.writePos += n;
  }

  private writeSilence(count: number): void {
    let left = Math.min(count, this.capacity);
    while (left > 0) {
      const n = Math.min(left, this.zeros.length);
      this.write(this.zeros.subarray(0, n));
      left -= n;
    }
  }

  private at(i: number): number {
    return i < 0 ? 0 : (this.ring[i % this.capacity] ?? 0);
  }

  /** Fill `out` with output-rate samples; silence while buffering or after an underrun. */
  pull(out: Float32Array): void {
    const n = out.length;
    if (this.state === "buffering") {
      if (this.fill() < this.target) {
        out.fill(0);
        return;
      }
      this.state = "playing";
      this.fillAvg = this.fill();
    }
    const dt = n / this.outputRate;
    this.fillAvg += (this.fill() - this.fillAvg) * Math.min(1, dt / FILL_TAU_S);
    const err = (this.fillAvg - this.target) / this.target;
    this.integral = clamp(this.integral + KI * err * dt, MAX_RATIO_ADJ);
    this.adj = clamp(KP * err + this.integral, MAX_RATIO_ADJ);
    const step = this.nominalStep * (1 + this.adj);

    let pos = this.readPos;
    for (let i = 0; i < n; i++) {
      const idx = Math.floor(pos);
      // The cubic needs samples idx-1 .. idx+2.
      if (idx + 2 >= this.writePos) {
        out.fill(0, i);
        this.state = "buffering";
        this.underruns += 1;
        break;
      }
      const t = pos - idx;
      const p0 = this.at(idx - 1);
      const p1 = this.at(idx);
      const p2 = this.at(idx + 1);
      const p3 = this.at(idx + 2);
      const a = -0.5 * p0 + 1.5 * p1 - 1.5 * p2 + 0.5 * p3;
      const b = p0 - 2.5 * p1 + 2 * p2 - 0.5 * p3;
      const c = -0.5 * p0 + 0.5 * p2;
      out[i] = ((a * t + b) * t + c) * t + p1;
      pos += step;
    }
    this.readPos = pos;
  }
}

function clamp(v: number, limit: number): number {
  return v > limit ? limit : v < -limit ? -limit : v;
}
