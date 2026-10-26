// How a band plan channel or bookmark tunes a VFO. A bookmark also carries squelch and a CTCSS tone
// squelch; those are applied with the tuning (the tone only to NFM / AM, the modes that have it).

import type { AnnotationTarget } from "../spectrum/annotations";
import type { Vfo, VfoCreate, VfoMode, VfoPatch } from "../vfo/api";
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
    ...(t.squelchDbfs !== undefined ? { squelchDbfs: t.squelchDbfs } : {}),
    ...(hasTones(t.mode ?? vfo.mode) && t.toneSquelchHz !== undefined
      ? { toneSquelchHz: t.toneSquelchHz }
      : {}),
  };
}

/** Only NFM and AM have CTCSS detection and tone squelch. */
const hasTones = (mode: VfoMode): boolean => mode === "nfm" || mode === "am";

/** A new VFO for the target; without a mode the usual one for the frequency. */
export function createFor(t: AnnotationTarget): VfoCreate {
  return {
    freqHz: t.freqHz,
    mode: t.mode ?? defaultModeFor(t.freqHz),
    ...(t.bandwidthHz !== undefined ? { bandwidthHz: t.bandwidthHz } : {}),
    ...(t.squelchDbfs !== undefined ? { squelchDbfs: t.squelchDbfs } : {}),
    ...(typeof t.toneSquelchHz === "number" && hasTones(t.mode ?? defaultModeFor(t.freqHz))
      ? { toneSquelchHz: t.toneSquelchHz }
      : {}),
  };
}
