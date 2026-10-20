// Volume/latency helpers (pure).

/** Slider position 0..1 to linear amplitude (squared curve feels closer to loudness). */
export function volumeToGain(volume: number, muted = false): number {
  if (muted || !Number.isFinite(volume)) return 0;
  const v = Math.min(1, Math.max(0, volume));
  return v * v;
}

/** Capture to arrival in the browser; includes the clock skew between server and browser. */
export function transitMs(nowMs: number, captureUnixNs: number): number {
  return nowMs - captureUnixNs / 1e6;
}

export interface LatencyEstimate {
  transitMs: number;
  /** Audio buffered in the jitter buffer. */
  bufferMs: number;
  /** AudioContext base + output latency. */
  outputMs: number;
  totalMs: number;
}

/** Capture-to-speaker estimate; accuracy is limited by clock skew and block sizes. */
export function estimateLatency(
  transit: number,
  bufferMs: number,
  outputMs: number,
): LatencyEstimate {
  return { transitMs: transit, bufferMs, outputMs, totalMs: transit + bufferMs + outputMs };
}
