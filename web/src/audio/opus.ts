// Per-VFO Opus decoding with WebCodecs (mono or stereo, following the frames). A WASM fallback
// (for browsers without AudioDecoder, e.g. older Firefox/Safari releases) is a later step.

import { AUDIO_SAMPLE_RATE, type AudioFrame } from "./frame";

export function opusSupported(): boolean {
  return typeof AudioDecoder !== "undefined" && typeof EncodedAudioChunk !== "undefined";
}

/** Drop frames instead of queueing when the decoder falls this far behind. */
const MAX_DECODE_QUEUE = 10;

export interface OpusStreamOptions {
  /** `right` is set for stereo frames; `samples` is the left (or only) channel. */
  onPcm: (samples: Float32Array, samplePos: number, right?: Float32Array) => void;
  onError?: (message: string) => void;
}

export class OpusStream {
  private decoder: AudioDecoder | null = null;
  private channels = 0;
  /** Sample positions of frames handed to the decoder, in output order. */
  private positions: number[] = [];
  private closed = false;

  constructor(private readonly opts: OpusStreamOptions) {}

  private ensureDecoder(channels: number): AudioDecoder {
    if (this.decoder && this.decoder.state === "configured" && this.channels === channels) {
      return this.decoder;
    }
    // First frame, an error, or a mono/stereo switch of the stream: start a fresh decoder.
    if (this.decoder && this.decoder.state !== "closed") this.decoder.close();
    this.positions = [];
    this.channels = channels;
    const decoder = new AudioDecoder({
      output: (data) => {
        const pos = this.positions.shift();
        try {
          const out = new Float32Array(data.numberOfFrames);
          data.copyTo(out, { planeIndex: 0, format: "f32-planar" });
          let right: Float32Array | undefined;
          if (data.numberOfChannels >= 2) {
            right = new Float32Array(data.numberOfFrames);
            data.copyTo(right, { planeIndex: 1, format: "f32-planar" });
          }
          if (pos !== undefined && !this.closed) this.opts.onPcm(out, pos, right);
        } finally {
          data.close();
        }
      },
      error: (e) => {
        this.opts.onError?.(`Opus decoder: ${e.message}`);
        this.positions = [];
      },
    });
    decoder.configure({ codec: "opus", sampleRate: AUDIO_SAMPLE_RATE, numberOfChannels: channels });
    this.decoder = decoder;
    return decoder;
  }

  decode(frame: AudioFrame): void {
    if (this.closed) return;
    const decoder = this.ensureDecoder(frame.channels);
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
