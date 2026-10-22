// Pure VFO helpers: frequency snapping, defaults, squelch slider mapping, hit testing.

import type { Vfo, VfoMode } from "./api";

export const MODE_LABEL: Record<VfoMode, string> = { wfm: "WFM", nfm: "NFM", am: "AM" };

/** Default tuning step per mode (Hz). */
export const DEFAULT_STEP_HZ: Record<VfoMode, number> = { wfm: 10_000, nfm: 12_500, am: 5_000 };

/** Selectable steps; "auto" (the mode default) is handled by the UI. */
export const STEPS_HZ = [100, 1_000, 5_000, 6_250, 10_000, 12_500, 25_000, 100_000] as const;

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

/** Sensible mode for a new VFO: FM broadcast WFM, airband AM, everything else NFM. */
export function defaultModeFor(hz: number): VfoMode {
  if (hz >= 87.5e6 && hz <= 108e6) return "wfm";
  if (hz > 108e6 && hz <= 137e6) return "am";
  return "nfm";
}

/** Bandwidth presets (Hz) offered in the panel; the mode default is the empty choice. */
export const BANDWIDTH_PRESETS: Record<VfoMode, readonly number[]> = {
  wfm: [100_000, 150_000, 180_000, 200_000],
  nfm: [6_250, 8_500, 12_500, 15_000, 25_000],
  am: [5_000, 6_000, 8_000, 10_000, 12_000],
};

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
 * The VFO under a frequency: inside its bandwidth, or within `slopHz` of its centre line for
 * narrow VFOs. Ties go to the VFO with the nearest centre.
 */
export function hitTestVfo(markers: readonly Marker[], hz: number, slopHz: number): Marker | null {
  let best: Marker | null = null;
  let bestDist = Infinity;
  for (const m of markers) {
    const dist = Math.abs(hz - m.freqHz);
    if (dist <= Math.max(m.bandwidthHz / 2, slopHz) && dist < bestDist) {
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
