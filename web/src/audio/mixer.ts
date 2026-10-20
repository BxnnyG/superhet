// Sums the jitter buffers of all VFOs into one output block. Pure and worklet-agnostic.

import { JitterBuffer, DEFAULT_TARGET_MS, type JitterStats } from "./jitter";

/** Per-sample gain smoothing (one-pole) to avoid clicks on volume/mute changes. */
const GAIN_SMOOTH = 0.002;

interface Channel {
  buffer: JitterBuffer;
  gain: number;
  targetGain: number;
}

export interface MixerOptions {
  outputRate: number;
  inputRate?: number;
  targetMs?: number;
}

export class Mixer {
  private readonly channels = new Map<number, Channel>();
  private targetMs: number;
  private scratch = new Float32Array(128);

  constructor(private readonly opts: MixerOptions) {
    this.targetMs = opts.targetMs ?? DEFAULT_TARGET_MS;
  }

  has(id: number): boolean {
    return this.channels.has(id);
  }

  add(id: number, gain = 1): void {
    if (this.channels.has(id)) return;
    this.channels.set(id, {
      buffer: new JitterBuffer({
        inputRate: this.opts.inputRate,
        outputRate: this.opts.outputRate,
        targetMs: this.targetMs,
      }),
      gain,
      targetGain: gain,
    });
  }

  remove(id: number): void {
    this.channels.delete(id);
  }

  setGain(id: number, gain: number): void {
    const ch = this.channels.get(id);
    if (ch) ch.targetGain = gain;
  }

  setTargetMs(ms: number): void {
    this.targetMs = ms;
    for (const ch of this.channels.values()) ch.buffer.setTargetMs(ms);
  }

  push(id: number, samples: Float32Array, samplePos?: number): void {
    this.channels.get(id)?.buffer.push(samples, samplePos);
  }

  /** Render one block (mono). Output is clipped to [-1, 1]. */
  render(out: Float32Array): void {
    const n = out.length;
    out.fill(0);
    if (this.scratch.length < n) this.scratch = new Float32Array(n);
    const tmp = this.scratch.subarray(0, n);
    for (const ch of this.channels.values()) {
      ch.buffer.pull(tmp);
      let g = ch.gain;
      const target = ch.targetGain;
      for (let i = 0; i < n; i++) {
        g += (target - g) * GAIN_SMOOTH;
        out[i] = (out[i] ?? 0) + (tmp[i] ?? 0) * g;
      }
      ch.gain = Math.abs(target - g) < 1e-4 ? target : g;
    }
    for (let i = 0; i < n; i++) {
      const v = out[i] ?? 0;
      out[i] = v > 1 ? 1 : v < -1 ? -1 : v;
    }
  }

  stats(): Array<{ id: number } & JitterStats> {
    return [...this.channels].map(([id, ch]) => ({ id, ...ch.buffer.stats() }));
  }
}
