// Pure VFO helpers: frequency snapping, defaults, squelch slider mapping, hit testing.

import type { Vfo, VfoMode } from "./api";

export const MODE_LABEL: Record<VfoMode, string> = {
  wfm: "WFM",
  nfm: "NFM",
  am: "AM",
  usb: "USB",
  lsb: "LSB",
  cw: "CW",
};

/** Default bandwidth per mode (Hz), as the daemon applies it when no override is set. */
export const DEFAULT_BANDWIDTH_HZ: Record<VfoMode, number> = {
  wfm: 180_000,
  nfm: 12_500,
  am: 10_000,
  usb: 2_400,
  lsb: 2_400,
  cw: 500,
};

/**
 * Lower audio edge of the USB/LSB passband (Hz from the carrier): the passband spans
 * `SSB_LOW_CUT_HZ` .. `SSB_LOW_CUT_HZ + bandwidth` on one side of the VFO frequency.
 */
export const SSB_LOW_CUT_HZ = 300;

/** Frequency range the receiver passes for a VFO: one-sided for USB/LSB, centred otherwise. */
export function passbandRange(
  mode: VfoMode,
  freqHz: number,
  bandwidthHz: number,
): { loHz: number; hiHz: number } {
  if (mode === "usb")
    return { loHz: freqHz + SSB_LOW_CUT_HZ, hiHz: freqHz + SSB_LOW_CUT_HZ + bandwidthHz };
  if (mode === "lsb")
    return { loHz: freqHz - SSB_LOW_CUT_HZ - bandwidthHz, hiHz: freqHz - SSB_LOW_CUT_HZ };
  return { loHz: freqHz - bandwidthHz / 2, hiHz: freqHz + bandwidthHz / 2 };
}

/** Default tuning step per mode (Hz). */
export const DEFAULT_STEP_HZ: Record<VfoMode, number> = {
  wfm: 10_000,
  nfm: 12_500,
  am: 5_000,
  usb: 100,
  lsb: 100,
  cw: 50,
};

/** Selectable steps; "auto" (the mode default) is handled by the UI. */
export const STEPS_HZ = [
  10, 50, 100, 500, 1_000, 5_000, 6_250, 10_000, 12_500, 25_000, 100_000,
] as const;

export type StepChoice = "auto" | number;

export function stepFor(choice: StepChoice, mode: VfoMode): number {
  return choice === "auto" ? DEFAULT_STEP_HZ[mode] : choice;
}

export function formatStep(hz: number): string {
  return hz >= 1000 ? `${parseFloat((hz / 1000).toFixed(3))} kHz` : `${hz} Hz`;
}

/** Snap to the nearest multiple of `stepHz`; never below one step. */
export function snapFrequency(hz: number, stepHz: number): number {
  if (!(stepHz > 0)) return Math.round(hz);
  return Math.max(stepHz, Math.round(hz / stepHz) * stepHz);
}

/**
 * HF amateur bands (Hz, IARU region 1 plus the common US edges). Operators use single sideband
 * there: LSB below 10 MHz, USB from 10 MHz up. Keep in sync with `default_mode` in
 * `crates/superhet-sdrd/src/controller.rs`.
 */
const HAM_HF_BANDS: ReadonlyArray<readonly [number, number]> = [
  [1.8e6, 2e6],
  [3.5e6, 3.9e6],
  [5.25e6, 5.45e6],
  [7e6, 7.2e6],
  [10.1e6, 10.15e6],
  [14e6, 14.35e6],
  [18.068e6, 18.168e6],
  [21e6, 21.45e6],
  [24.89e6, 24.99e6],
  [28e6, 29.7e6],
];

/**
 * Sensible mode for a new VFO when the user (or a bookmark / band plan entry) did not choose
 * one: FM broadcast WFM, airband AM, HF amateur bands LSB (below 10 MHz) or USB, the rest of
 * HF (broadcast, CB, utility) AM, everything else NFM.
 */
export function defaultModeFor(hz: number): VfoMode {
  if (hz >= 87.5e6 && hz <= 108e6) return "wfm";
  if (hz > 108e6 && hz <= 137e6) return "am";
  if (hz < 30e6) {
    if (HAM_HF_BANDS.some(([lo, hi]) => hz >= lo && hz <= hi)) return hz < 10e6 ? "lsb" : "usb";
    return "am";
  }
  return "nfm";
}

/** Bandwidth presets (Hz) offered in the panel; the mode default is the empty choice. */
export const BANDWIDTH_PRESETS: Record<VfoMode, readonly number[]> = {
  wfm: [100_000, 150_000, 180_000, 200_000],
  nfm: [6_250, 8_500, 12_500, 15_000, 25_000],
  am: [5_000, 6_000, 8_000, 10_000, 12_000],
  usb: [1_800, 2_100, 2_400, 2_700, 3_000, 3_600],
  lsb: [1_800, 2_100, 2_400, 2_700, 3_000, 3_600],
  cw: [100, 200, 300, 500, 800, 1_200, 2_000],
};

/** Smallest and largest bandwidth offered for a mode (the ends of its presets). */
export function bandwidthLimits(mode: VfoMode): { min: number; max: number } {
  const presets = BANDWIDTH_PRESETS[mode];
  return { min: presets[0] ?? 0, max: presets[presets.length - 1] ?? 0 };
}

/** The frequency `steps` tuning steps away (negative = down), on the step grid. */
export function stepFrequency(hz: number, stepHz: number, steps: number): number {
  return snapFrequency(hz + steps * stepHz, stepHz);
}

export function formatBandwidth(hz: number): string {
  return hz >= 1000 ? `${parseFloat((hz / 1000).toFixed(2))} kHz` : `${hz} Hz`;
}

/** Slider range: SQUELCH_OFF_POS means "off", then -120..0 dBFS. */
export const SQUELCH_MIN_DBFS = -120;
export const SQUELCH_OFF_POS = SQUELCH_MIN_DBFS - 1;

export function squelchToSlider(dbfs: number | null): number {
  return dbfs === null
    ? SQUELCH_OFF_POS
    : Math.min(0, Math.max(SQUELCH_MIN_DBFS, Math.round(dbfs)));
}

export function sliderToSquelch(pos: number): number | null {
  return pos <= SQUELCH_OFF_POS ? null : Math.min(0, pos);
}

/** Window of channel-level history the squelch "Auto" looks at. */
export const AUTO_SQUELCH_WINDOW_MS = 3000;
/** Fewer samples than this are too little to judge the noise floor. */
export const AUTO_SQUELCH_MIN_SAMPLES = 5;
const AUTO_SQUELCH_PERCENTILE = 0.2;
const AUTO_SQUELCH_MARGIN_DB = 4;

/**
 * Squelch threshold (dBFS) from recent channel powers: the 20th percentile approximates the
 * noise floor even when a transmission was in the window; a 4 dB margin keeps noise below the
 * threshold. Clamped to the slider range; null when there is too little data.
 */
export function autoSquelchDbfs(levels: readonly number[]): number | null {
  const sorted = levels.filter(Number.isFinite).sort((a, b) => a - b);
  if (sorted.length < AUTO_SQUELCH_MIN_SAMPLES) return null;
  const floor = sorted[Math.floor((sorted.length - 1) * AUTO_SQUELCH_PERCENTILE)] ?? 0;
  return Math.min(0, Math.max(SQUELCH_MIN_DBFS, Math.round(floor + AUTO_SQUELCH_MARGIN_DB)));
}

export function formatSquelch(dbfs: number | null): string {
  return dbfs === null ? "off" : `${Math.round(dbfs)} dBFS`;
}

/** S-meter bar fraction (0..1) for a channel power in dBFS (-120..0). */
export function meterFraction(powerDbfs: number | null): number {
  if (powerDbfs === null || !Number.isFinite(powerDbfs)) return 0;
  return Math.min(1, Math.max(0, (powerDbfs - SQUELCH_MIN_DBFS) / -SQUELCH_MIN_DBFS));
}

const VFO_COLORS = [
  "#38bdf8",
  "#f472b6",
  "#a3e635",
  "#fb923c",
  "#c084fc",
  "#2dd4bf",
  "#facc15",
  "#f87171",
];

export function vfoColor(id: number): string {
  return VFO_COLORS[(Math.max(1, id) - 1) % VFO_COLORS.length] ?? "#38bdf8";
}

export interface Marker {
  id: number;
  freqHz: number;
  bandwidthHz: number;
  mode: VfoMode;
  paused: boolean;
}

export function toMarker(v: Vfo): Marker {
  return {
    id: v.id,
    freqHz: v.freqHz,
    bandwidthHz: v.bandwidthHz,
    mode: v.mode,
    paused: v.status === "paused",
  };
}

/**
 * The VFO under a frequency: inside its passband, or within `slopHz` of its centre line for
 * narrow VFOs (and for the carrier of single-sideband VFOs, which lies outside their passband). Ties go to the VFO with the nearest centre.
 */
export function hitTestVfo(markers: readonly Marker[], hz: number, slopHz: number): Marker | null {
  let best: Marker | null = null;
  let bestDist = Infinity;
  for (const m of markers) {
    const dist = Math.abs(hz - m.freqHz);
    const { loHz, hiHz } = passbandRange(m.mode, m.freqHz, m.bandwidthHz);
    if (dist < bestDist && ((hz >= loHz && hz <= hiHz) || dist <= slopHz)) {
      best = m;
      bestDist = dist;
    }
  }
  return best;
}

/** Shown when a VFO sits outside the usable window while the tuning lock keeps the centre. */
export const VFO_PAUSED_HINT = "Tuning locked — VFO paused outside the window";
/** Hint for a paused VFO while free tuning is on (no lock). */
export const VFO_PAUSED_FREE_HINT =
  "VFO outside the usable window — paused until the device is retuned";
