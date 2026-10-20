// Per-VFO Opus decoding with WebCodecs. A WASM fallback (for browsers without AudioDecoder,
// e.g. older Firefox/Safari releases) is a later step.

import { AUDIO_SAMPLE_RATE, type AudioFrame } from "./frame";

export function opusSupported(): boolean {
  return typeof AudioDecoder !== "undefined" && typeof EncodedAudioChunk !== "undefined";
}

/** Drop frames instead of queueing when the decoder falls this far behind. */
const MAX_DECODE_QUEUE = 10;

export interface OpusStreamOptions {
  onPcm: (samples: Float32Array, samplePos: number) => void;
  onError?: (message: string) => void;
}

export class OpusStream {
  private decoder: AudioDecoder | null = null;
  /** Sample positions of frames handed to the decoder, in output order. */
  private positions: number[] = [];
  private closed = false;

  constructor(private readonly opts: OpusStreamOptions) {}

  private ensureDecoder(): AudioDecoder {
    if (this.decoder && this.decoder.state === "configured") return this.decoder;
    this.decoder?.close();
    this.positions = [];
    const decoder = new AudioDecoder({
      output: (data) => {
        const pos = this.positions.shift();
        try {
          const out = new Float32Array(data.numberOfFrames);
          data.copyTo(out, { planeIndex: 0, format: "f32-planar" });
          if (pos !== undefined && !this.closed) this.opts.onPcm(out, pos);
        } finally {
          data.close();
        }
      },
      error: (e) => {
        this.opts.onError?.(`Opus decoder: ${e.message}`);
        this.positions = [];
      },
    });
    decoder.configure({ codec: "opus", sampleRate: AUDIO_SAMPLE_RATE, numberOfChannels: 1 });
    this.decoder = decoder;
    return decoder;
  }

  decode(frame: AudioFrame): void {
    if (this.closed) return;
    const decoder = this.ensureDecoder();
    if (decoder.decodeQueueSize > MAX_DECODE_QUEUE) return;
    this.positions.push(frame.samplePos);
    decoder.decode(
      new EncodedAudioChunk({
        type: "key",
        timestamp: Math.round((frame.samplePos * 1e6) / AUDIO_SAMPLE_RATE),
        data: frame.payload,
      }),
    );
  }

  close(): void {
    this.closed = true;
    if (this.decoder && this.decoder.state !== "closed") this.decoder.close();
    this.decoder = null;
    this.positions = [];
  }
}
