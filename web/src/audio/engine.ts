// Audio output: one AudioContext, one AudioWorklet mixer, one Opus decoder per VFO.
// Everything per-frame stays off React; the controller reads `debug()` at a low rate.

import { AUDIO_SAMPLE_RATE, type AudioFrame } from "./frame";
import { estimateLatency, transitMs } from "./gain";
import { DEFAULT_TARGET_MS, type JitterStats } from "./jitter";
import { OpusStream, opusSupported } from "./opus";
import { WORKLET_NAME, type FromWorklet, type ToWorklet } from "./protocol";

export interface EngineSupport {
  ok: boolean;
  /** Human-readable reason when `ok` is false. */
  reason?: string;
}

export function checkSupport(): EngineSupport {
  if (typeof AudioContext === "undefined" || typeof AudioWorkletNode === "undefined") {
    return { ok: false, reason: "This browser has no AudioWorklet support." };
  }
  if (!opusSupported()) {
    return {
      ok: false,
      reason:
        "This browser has no WebCodecs Opus decoder (AudioDecoder). Use a current Chrome, Edge " +
        "or Firefox; a WASM fallback decoder is planned.",
    };
  }
  return { ok: true };
}

export interface VfoAudioDebug extends JitterStats {
  id: number;
  /** Smoothed capture-to-browser delay in ms (includes clock skew); null before the first frame. */
  transitMs: number | null;
  /** Estimated capture-to-speaker latency in ms. */
  totalMs: number | null;
}

export interface EngineDebug {
  contextRate: number;
  outputMs: number;
  vfos: VfoAudioDebug[];
}

interface Stream {
  opus: OpusStream;
  transitMs: number | null;
}

const TRANSIT_EMA = 0.05;

export class AudioEngine {
  private ctx: AudioContext | null = null;
  private node: AudioWorkletNode | null = null;
  private readonly streams = new Map<number, Stream>();
  private readonly gains = new Map<number, number>();
  private stats = new Map<number, JitterStats>();
  private targetMs = DEFAULT_TARGET_MS;
  private starting: Promise<void> | null = null;

  constructor(private readonly onError: (message: string) => void = () => undefined) {}

  get running(): boolean {
    return this.node !== null;
  }

  /** Create/resume the context. Must be called from a user gesture (autoplay policy). */
  enable(): Promise<void> {
    if (this.node && this.ctx) return this.ctx.resume();
    this.starting ??= this.start().finally(() => {
      this.starting = null;
    });
    return this.starting;
  }

  private async start(): Promise<void> {
    const support = checkSupport();
    if (!support.ok) throw new Error(support.reason);
    let ctx: AudioContext;
    try {
      ctx = new AudioContext({ sampleRate: AUDIO_SAMPLE_RATE, latencyHint: "interactive" });
    } catch {
      // The hardware rate is not selectable: the mixer resamples from 48 kHz instead.
      ctx = new AudioContext({ latencyHint: "interactive" });
    }
    try {
      await ctx.resume();
      const { default: url } = await import("./worklet.ts?worker&url");
      await ctx.audioWorklet.addModule(url);
      const node = new AudioWorkletNode(ctx, WORKLET_NAME, {
        numberOfInputs: 0,
        numberOfOutputs: 1,
        outputChannelCount: [2],
      });
      node.port.onmessage = (ev: MessageEvent<FromWorklet>) => {
        if (ev.data.type !== "stats") return;
        this.stats = new Map(ev.data.vfos.map((v) => [v.id, v]));
      };
      node.connect(ctx.destination);
      this.ctx = ctx;
      this.node = node;
      this.post({ type: "config", targetMs: this.targetMs, inputRate: AUDIO_SAMPLE_RATE });
      for (const id of this.streams.keys()) this.addToWorklet(id);
    } catch (e) {
      void ctx.close();
      throw e;
    }
  }

  async disable(): Promise<void> {
    for (const id of [...this.streams.keys()]) this.dropStream(id);
    this.node?.disconnect();
    if (this.node) this.node.port.onmessage = null;
    this.node = null;
    this.stats = new Map();
    const ctx = this.ctx;
    this.ctx = null;
    await ctx?.close();
  }

  private post(msg: ToWorklet, transfer: Transferable[] = []): void {
    this.node?.port.postMessage(msg, transfer);
  }

  private addToWorklet(id: number): void {
    this.post({ type: "add", vfo: id, gain: this.gains.get(id) ?? 1 });
  }

  /** Effective linear gain (volume and mute already applied). */
  setGain(id: number, gain: number): void {
    this.gains.set(id, gain);
    this.post({ type: "gain", vfo: id, gain });
  }

  setTargetMs(ms: number): void {
    this.targetMs = ms;
    this.post({ type: "target", ms });
  }

  /** Keep streams for exactly these VFO ids. */
  setVfos(ids: Iterable<number>): void {
    const keep = new Set(ids);
    for (const id of [...this.streams.keys()]) if (!keep.has(id)) this.dropStream(id);
    for (const id of [...this.gains.keys()]) if (!keep.has(id)) this.gains.delete(id);
  }

  private dropStream(id: number): void {
    const s = this.streams.get(id);
    if (!s) return;
    s.opus.close();
    this.streams.delete(id);
    this.stats.delete(id);
    this.post({ type: "remove", vfo: id });
  }

  onFrame(frame: AudioFrame): void {
    if (!this.node) return;
    let s = this.streams.get(frame.vfoId);
    if (!s) {
      const stream: Stream = {
        opus: new OpusStream({
          onPcm: (samples, pos) =>
            this.post({ type: "pcm", vfo: frame.vfoId, pos, samples }, [samples.buffer]),
          onError: this.onError,
        }),
        transitMs: null,
      };
      this.streams.set(frame.vfoId, stream);
      this.addToWorklet(frame.vfoId);
      s = stream;
    }
    const transit = transitMs(Date.now(), frame.captureUnixNs);
    s.transitMs =
      s.transitMs === null ? transit : s.transitMs + (transit - s.transitMs) * TRANSIT_EMA;
    s.opus.decode(frame);
  }

  debug(): EngineDebug | null {
    const ctx = this.ctx;
    if (!ctx) return null;
    const outputMs = ((ctx.baseLatency || 0) + (ctx.outputLatency || 0)) * 1000;
    const vfos: VfoAudioDebug[] = [];
    for (const [id, st] of this.stats) {
      const transit = this.streams.get(id)?.transitMs ?? null;
      const totalMs =
        transit === null ? null : estimateLatency(transit, st.fillMs, outputMs).totalMs;
      vfos.push({ ...st, id, transitMs: transit, totalMs });
    }
    return { contextRate: ctx.sampleRate, outputMs, vfos };
  }
}
