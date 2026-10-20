// Decoder for the binary audio frame, wire format v1 (docs/protocol-audio.md).
// Layout reference: crates/superhet-bus/src/audio.rs.

export const AUDIO_HEADER_LEN = 42;
export const AUDIO_VERSION = 1;
export const CODEC_OPUS = 1;
export const AUDIO_SAMPLE_RATE = 48_000;
/** Samples per 20 ms Opus packet. */
export const SAMPLES_PER_FRAME = 960;

export interface AudioFrame {
  vfoId: number;
  seq: number;
  flags: number;
  /** Squelch open, or no squelch set (flag bit 0). */
  squelchOpen: boolean;
  sampleRate: number;
  channels: number;
  /** 48 kHz sample counter of the first sample. */
  samplePos: number;
  /** Wall time (ns since the Unix epoch) at which the IQ of the first sample was captured. */
  captureUnixNs: number;
  /** Channel power (S-meter), 0 dBFS = complex tone at full scale. */
  powerDbfs: number;
  /** One Opus packet; a view into the received buffer. */
  payload: Uint8Array;
}

export class AudioFrameError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "AudioFrameError";
  }
}

const MAGIC = [0x53, 0x48, 0x41, 0x55]; // "SHAU"

export function decodeAudioFrame(data: ArrayBuffer | ArrayBufferView): AudioFrame {
  const bytes =
    data instanceof ArrayBuffer
      ? new Uint8Array(data)
      : new Uint8Array(data.buffer, data.byteOffset, data.byteLength);
  if (bytes.length < AUDIO_HEADER_LEN) {
    throw new AudioFrameError(`truncated audio header: ${bytes.length} bytes`);
  }
  if (MAGIC.some((b, i) => bytes[i] !== b)) throw new AudioFrameError("bad audio magic");
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const version = view.getUint8(4);
  if (version !== AUDIO_VERSION) throw new AudioFrameError(`unsupported audio version ${version}`);
  const codec = view.getUint8(5);
  if (codec !== CODEC_OPUS) throw new AudioFrameError(`unsupported audio codec ${codec}`);
  const payloadLen = view.getUint16(40, true);
  if (bytes.length !== AUDIO_HEADER_LEN + payloadLen) {
    throw new AudioFrameError(
      `audio length mismatch: expected ${AUDIO_HEADER_LEN + payloadLen} bytes, got ${bytes.length}`,
    );
  }
  const flags = view.getUint8(7);
  return {
    vfoId: view.getUint32(12, true),
    seq: view.getUint32(16, true),
    flags,
    squelchOpen: (flags & 1) !== 0,
    sampleRate: view.getUint32(8, true),
    channels: view.getUint8(6),
    // Sample counters stay below 2^53 for thousands of years. Epoch nanoseconds exceed 2^53 and
    // are rounded to ~256 ns, which is irrelevant for latency measurement.
    samplePos: Number(view.getBigUint64(20, true)),
    captureUnixNs: Number(view.getBigUint64(28, true)),
    powerDbfs: view.getFloat32(36, true),
    payload: bytes.subarray(AUDIO_HEADER_LEN),
  };
}
