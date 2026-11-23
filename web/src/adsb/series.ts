// Altitude, speed and climb-rate history of each aircraft, kept in the browser for as long as it
// stays on the picture (the plugin only reports current values). It feeds the panel's charts;
// nothing is sent anywhere. Unlike the trails it also records aircraft without a position.

import type { Aircraft } from "./model";

export interface Sample {
  /** When the picture arrived, ms since epoch. */
  t: number;
  altFt: number | null;
  onGround: boolean;
  speedKt: number | null;
  vrateFpm: number | null;
}

/** Two samples of one aircraft are at least this far apart in time. */
export const SAMPLE_STEP_MS = 5000;
/** History kept per aircraft: what the longest trail keeps. */
export const SERIES_WINDOW_MS = 30 * 60_000;
/** Hard cap per aircraft (30 minutes at one sample per step, with some slack). */
export const MAX_SAMPLES = 400;

export class SeriesStore {
  private readonly series = new Map<string, Sample[]>();

  /** Take one status picture: sample every aircraft and forget those no longer listed. */
  record(aircraft: readonly Aircraft[], receivedAtMs: number): void {
    const listed = new Set<string>();
    for (const a of aircraft) {
      listed.add(a.icao);
      let samples = this.series.get(a.icao);
      if (samples === undefined) {
        samples = [];
        this.series.set(a.icao, samples);
      }
      const last = samples[samples.length - 1];
      if (last !== undefined && receivedAtMs - last.t < SAMPLE_STEP_MS) continue;
      samples.push({
        t: receivedAtMs,
        altFt: a.altFt,
        onGround: a.onGround,
        speedKt: a.speedKt,
        vrateFpm: a.vrateFpm,
      });
      if (samples.length > MAX_SAMPLES) samples.splice(0, samples.length - MAX_SAMPLES);
      while (samples[0] !== undefined && receivedAtMs - samples[0].t > SERIES_WINDOW_MS) {
        samples.shift();
      }
    }
    for (const icao of this.series.keys()) if (!listed.has(icao)) this.series.delete(icao);
  }

  get(icao: string): readonly Sample[] {
    return this.series.get(icao) ?? [];
  }

  clear(): void {
    this.series.clear();
  }
}
