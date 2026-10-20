// AudioWorklet processor: mixes the per-VFO jitter buffers. Bundled by Vite through
// `?worker&url` (see engine.ts). The globals below exist only in AudioWorkletGlobalScope.

import { Mixer } from "./mixer";
import { WORKLET_NAME, type FromWorklet, type ToWorklet } from "./protocol";

declare const sampleRate: number;
declare class AudioWorkletProcessor {
  readonly port: MessagePort;
}
declare function registerProcessor(name: string, ctor: new () => AudioWorkletProcessor): void;

const STATS_INTERVAL_S = 0.25;

class MixerProcessor extends AudioWorkletProcessor {
  private mixer = new Mixer({ outputRate: sampleRate });
  private statsAcc = 0;

  constructor() {
    super();
    this.port.onmessage = (ev: MessageEvent<ToWorklet>) => {
      const msg = ev.data;
      switch (msg.type) {
        case "config":
          this.mixer = new Mixer({
            outputRate: sampleRate,
            inputRate: msg.inputRate,
            targetMs: msg.targetMs,
          });
          break;
        case "add":
          this.mixer.add(msg.vfo, msg.gain);
          break;
        case "remove":
          this.mixer.remove(msg.vfo);
          break;
        case "gain":
          this.mixer.setGain(msg.vfo, msg.gain);
          break;
        case "target":
          this.mixer.setTargetMs(msg.ms);
          break;
        case "pcm":
          this.mixer.push(msg.vfo, msg.samples, msg.pos);
          break;
      }
    };
  }

  process(_inputs: Float32Array[][], outputs: Float32Array[][]): boolean {
    const out = outputs[0];
    const first = out?.[0];
    if (!out || !first) return true;
    this.mixer.render(first);
    for (let c = 1; c < out.length; c++) out[c]?.set(first);
    this.statsAcc += first.length / sampleRate;
    if (this.statsAcc >= STATS_INTERVAL_S) {
      this.statsAcc = 0;
      const msg: FromWorklet = { type: "stats", vfos: this.mixer.stats() };
      this.port.postMessage(msg);
    }
    return true;
  }
}

registerProcessor(WORKLET_NAME, MixerProcessor);
