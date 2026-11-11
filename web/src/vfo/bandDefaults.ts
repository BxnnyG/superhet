// Sensible mode, bandwidth and tuning step below 30 MHz. Used when a VFO is created without a
// mode and when a VFO that the user never switched by hand is retuned into another kind of band.
// Keep the mode rules in sync with `default_mode` in `crates/superhet-sdrd/src/controller.rs`.

import type { VfoMode } from "./api";
import { getMwRaster, type MwRaster } from "./mwRaster";

export interface BandDefault {
  mode: VfoMode;
  bandwidthHz: number;
  stepHz: number;
}

type Range = readonly [number, number];

/** Long-wave broadcast (ITU 1). */
const LW_BROADCAST: Range = [148_500, 283_500];
/** Medium-wave broadcast, wide enough for both rasters. */
const MW_BROADCAST: Range = [520_000, 1_710_000];

/** Shortwave broadcast bands. */
const SW_BROADCAST: readonly Range[] = [
  [2_300_000, 2_495_000],
  [3_200_000, 3_400_000],
  [3_900_000, 4_000_000],
  [4_750_000, 5_060_000],
  [5_900_000, 6_200_000],
  [7_200_000, 7_600_000],
  [9_400_000, 9_900_000],
  [11_600_000, 12_100_000],
  [13_570_000, 13_870_000],
  [15_100_000, 15_800_000],
  [17_480_000, 17_900_000],
  [18_900_000, 19_020_000],
  [21_450_000, 21_850_000],
  [25_670_000, 26_100_000],
];

/** Time signal stations (WWV / WWVH, BPM, RWM ...): AM +-5 kHz around the nominal frequency. */
const TIME_SIGNALS_HZ = [2.5e6, 10e6, 15e6, 20e6, 25e6];

/** 11 m citizens band. */
const CB: Range = [26_965_000, 27_405_000];

/** CW segments of the HF amateur bands (IARU region 1 band plan, the common core). */
const CW_SEGMENTS: readonly Range[] = [
  [1_800_000, 1_840_000],
  [3_500_000, 3_570_000],
  [7_000_000, 7_040_000],
  [10_100_000, 10_130_000],
  [14_000_000, 14_070_000],
  [18_068_000, 18_095_000],
  [21_000_000, 21_070_000],
  [24_890_000, 24_915_000],
  [28_000_000, 28_070_000],
];

/** Amateur HF bands (SSB segments live inside them). */
const HAM_BANDS: readonly Range[] = [
  [1_800_000, 2_000_000],
  [3_500_000, 3_900_000],
  [5_250_000, 5_450_000],
  [7_000_000, 7_200_000],
  [10_100_000, 10_150_000],
  [14_000_000, 14_350_000],
  [18_068_000, 18_168_000],
  [21_000_000, 21_450_000],
  [24_890_000, 24_990_000],
  [28_000_000, 29_700_000],
];

/** Lowest and highest frequency the table speaks for. */
export const HF_DEFAULTS_MIN_HZ = 100_000;
export const HF_DEFAULTS_MAX_HZ = 30_000_000;

const BROADCAST_BW_HZ = 6_000;
const SSB_BW_HZ = 2_400;
const CW_BW_HZ = 500;

const inside = (hz: number, [lo, hi]: Range) => hz >= lo && hz <= hi;

/**
 * Defaults for a frequency: AM in the broadcast bands (9 or 10 kHz steps on medium wave, 5 kHz on
 * shortwave), CW in the amateur CW segments (500 Hz, 100 Hz steps), otherwise LSB below 10 MHz
 * and USB from 10 MHz up (2.4 kHz; 100 Hz steps in amateur bands, 1 kHz elsewhere). Null outside
 * 100 kHz..30 MHz.
 */
export function bandDefaultFor(hz: number, mwRaster: MwRaster = getMwRaster()): BandDefault | null {
  if (!Number.isFinite(hz) || hz < HF_DEFAULTS_MIN_HZ || hz >= HF_DEFAULTS_MAX_HZ) return null;
  if (inside(hz, LW_BROADCAST)) return { mode: "am", bandwidthHz: BROADCAST_BW_HZ, stepHz: 9_000 };
  if (inside(hz, MW_BROADCAST))
    return { mode: "am", bandwidthHz: BROADCAST_BW_HZ, stepHz: mwRaster };
  if (
    SW_BROADCAST.some((r) => inside(hz, r)) ||
    TIME_SIGNALS_HZ.some((f) => Math.abs(hz - f) <= 5_000)
  ) {
    return { mode: "am", bandwidthHz: BROADCAST_BW_HZ, stepHz: 5_000 };
  }
  if (inside(hz, CB)) return { mode: "am", bandwidthHz: BROADCAST_BW_HZ, stepHz: 10_000 };
  if (CW_SEGMENTS.some((r) => inside(hz, r))) {
    return { mode: "cw", bandwidthHz: CW_BW_HZ, stepHz: 100 };
  }
  return {
    mode: hz < 10_000_000 ? "lsb" : "usb",
    bandwidthHz: SSB_BW_HZ,
    stepHz: HAM_BANDS.some((r) => inside(hz, r)) ? 100 : 1_000,
  };
}

/**
 * The patch that moves a VFO into the defaults of its new frequency, or null when nothing should
 * change: the table has no entry, or the old and the new frequency have the same defaults (so
 * tuning around inside a band never fights the user's bandwidth).
 */
export function retuneDefaults(
  oldHz: number,
  newHz: number,
  mwRaster: MwRaster = getMwRaster(),
): { mode: VfoMode; bandwidthHz: number } | null {
  const next = bandDefaultFor(newHz, mwRaster);
  if (!next) return null;
  const prev = bandDefaultFor(oldHz, mwRaster);
  if (prev && prev.mode === next.mode && prev.bandwidthHz === next.bandwidthHz) return null;
  return { mode: next.mode, bandwidthHz: next.bandwidthHz };
}
