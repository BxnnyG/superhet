// How a band plan channel or bookmark tunes a VFO.

import type { AnnotationTarget } from "../spectrum/annotations";
import type { Vfo, VfoCreate, VfoPatch } from "../vfo/api";
import { defaultModeFor } from "../vfo/model";

/**
 * Patch for an existing VFO: always the frequency; a different mode switches it and takes the
 * target's bandwidth (null = the new mode's default); the same mode only changes the bandwidth
 * when the target names one, so a hand-set bandwidth survives tuning to a plain bookmark.
 */
export function patchFor(vfo: Pick<Vfo, "mode">, t: AnnotationTarget): VfoPatch {
  const modeChange = t.mode !== undefined && t.mode !== vfo.mode;
  return {
    freqHz: t.freqHz,
    ...(modeChange ? { mode: t.mode, bandwidthHz: t.bandwidthHz ?? null } : {}),
    ...(!modeChange && t.bandwidthHz !== undefined ? { bandwidthHz: t.bandwidthHz } : {}),
  };
}

/** A new VFO for the target; without a mode the usual one for the frequency. */
export function createFor(t: AnnotationTarget): VfoCreate {
  return {
    freqHz: t.freqHz,
    mode: t.mode ?? defaultModeFor(t.freqHz),
    ...(t.bandwidthHz !== undefined ? { bandwidthHz: t.bandwidthHz } : {}),
  };
}
