// Sums the jitter buffers of all VFOs into one (stereo) output block. Pure and worklet-agnostic.
// Every VFO has a left and a right jitter buffer that always see the same pushes (mono audio goes
// to both), so the two channels keep their timing and drift control in lockstep.

import { JitterBuffer, DEFAULT_TARGET_MS, type JitterStats } from "./jitter";

/** Per-sample gain smoothing (one-pole) to avoid clicks on volume/mute changes. */
const GAIN_SMOOTH = 0.002;

interface Channel {
  buffer: JitterBuffer;
  right: JitterBuffer;
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
  private scratchR = new Float32Array(128);

  constructor(private readonly opts: MixerOptions) {
    this.targetMs = opts.targetMs ?? DEFAULT_TARGET_MS;
  }

  has(id: number): boolean {
    return this.channels.has(id);
  }

  add(id: number, gain = 1): void {
    if (this.channels.has(id)) return;
    const jitter = () =>
      new JitterBuffer({
        inputRate: this.opts.inputRate,
        outputRate: this.opts.outputRate,
        targetMs: this.targetMs,
      });
    this.channels.set(id, {
      buffer: jitter(),
      right: jitter(),
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
    for (const ch of this.channels.values()) {
      ch.buffer.setTargetMs(ms);
      ch.right.setTargetMs(ms);
    }
  }

  /** Append decoded audio; `right` is given for stereo frames, mono audio feeds both channels. */
  push(id: number, samples: Float32Array, samplePos?: number, right?: Float32Array): void {
    const ch = this.channels.get(id);
    if (!ch) return;
    ch.buffer.push(samples, samplePos);
    ch.right.push(right ?? samples, samplePos);
  }

  /**
   * Render one block. With `outRight` the left and right channels are rendered separately;
   * without it `out` receives the mono downmix. Output is clipped to [-1, 1].
   */
  render(out: Float32Array, outRight?: Float32Array): void {
    const n = out.length;
    out.fill(0);
    outRight?.fill(0);
    if (this.scratch.length < n) {
      this.scratch = new Float32Array(n);
      this.scratchR = new Float32Array(n);
    }
    const left = this.scratch.subarray(0, n);
    const right = this.scratchR.subarray(0, n);
    for (const ch of this.channels.values()) {
      ch.buffer.pull(left);
      ch.right.pull(right);
      let g = ch.gain;
      const target = ch.targetGain;
      for (let i = 0; i < n; i++) {
        g += (target - g) * GAIN_SMOOTH;
        if (outRight) {
          out[i] = (out[i] ?? 0) + (left[i] ?? 0) * g;
          outRight[i] = (outRight[i] ?? 0) + (right[i] ?? 0) * g;
        } else {
          out[i] = (out[i] ?? 0) + 0.5 * ((left[i] ?? 0) + (right[i] ?? 0)) * g;
        }
      }
      ch.gain = Math.abs(target - g) < 1e-4 ? target : g;
    }
    clip(out);
    if (outRight) clip(outRight);
  }

  stats(): Array<{ id: number } & JitterStats> {
    return [...this.channels].map(([id, ch]) => ({ id, ...ch.buffer.stats() }));
  }
}

function clip(buf: Float32Array): void {
  for (let i = 0; i < buf.length; i++) {
    const v = buf[i] ?? 0;
    buf[i] = v > 1 ? 1 : v < -1 ? -1 : v;
  }
}
