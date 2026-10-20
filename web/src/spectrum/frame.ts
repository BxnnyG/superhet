// Decoder for the binary spectrum frame, wire format v1.
// Layout: see crates/superhet-bus/src/spectrum.rs and docs/protocol-ws.md.

export const HEADER_LEN = 32;
export const FRAME_VERSION = 1;
export const FORMAT_U8_DB = 0;

export interface SpectrumFrame {
  binCount: number;
  /** Spanned bandwidth in Hz; the frame covers centerHz ± sampleRate / 2. */
  sampleRate: number;
  centerHz: number;
  timestampUs: number;
  /** dBFS mapped to bin value 0. */
  minDb: number;
  /** dBFS mapped to bin value 255. */
  maxDb: number;
  /** One byte per bin, lowest frequency first. A view into the received buffer. */
  bins: Uint8Array;
}

export class FrameError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "FrameError";
  }
}

export function decodeFrame(data: ArrayBuffer | ArrayBufferView): SpectrumFrame {
  const bytes =
    data instanceof ArrayBuffer
      ? new Uint8Array(data)
      : new Uint8Array(data.buffer, data.byteOffset, data.byteLength);
  if (bytes.length < HEADER_LEN) {
    throw new FrameError(`truncated header: ${bytes.length} bytes`);
  }
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const version = view.getUint8(0);
  if (version !== FRAME_VERSION) {
    throw new FrameError(`unsupported frame version ${version}`);
  }
  const format = view.getUint8(1);
  if (format !== FORMAT_U8_DB) {
    throw new FrameError(`unsupported bin format ${format}`);
  }
  const binCount = view.getUint16(2, true);
  if (bytes.length !== HEADER_LEN + binCount) {
    throw new FrameError(
      `length mismatch: expected ${HEADER_LEN + binCount} bytes, got ${bytes.length}`,
    );
  }
  return {
    binCount,
    sampleRate: view.getUint32(4, true),
    // u64 values stay far below 2^53 (Hz up to 1 THz, µs timestamps until year 2255).
    centerHz: Number(view.getBigUint64(8, true)),
    timestampUs: Number(view.getBigUint64(16, true)),
    minDb: view.getFloat32(24, true),
    maxDb: view.getFloat32(28, true),
    bins: bytes.subarray(HEADER_LEN),
  };
}
