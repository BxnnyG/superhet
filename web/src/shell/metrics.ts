// Mutable counters shared between the spectrum renderer and the status bar. Written per frame
// without React; the status bar samples them at a low rate.

export class SpectrumMetrics {
  frames = 0;
}

/** Frames per second between two samples; 0 when no time has passed. */
export function fpsBetween(prevFrames: number, frames: number, dtMs: number): number {
  return dtMs > 0 ? Math.max(0, ((frames - prevFrames) * 1000) / dtMs) : 0;
}
