// Free channel bandwidth: the allowed range per mode and device sample rate, parsing of typed
// values ("12.5", "12.5k", "150 kHz", "8500 Hz"), snapping for drags and stepping for the wheel.
//
// The daemon accepts any whole number of Hz in 100..=1_000_000 and then asks the channelizer
// whether it can build the filter chain at the device rate (`check_realisable`); the rate limit
// below mirrors that rule conservatively so the UI rejects what the daemon would reject.

import type { VfoMode } from "./api";
import { bandwidthLimits, formatBandwidth, SSB_LOW_CUT_HZ } from "./model";

export interface BandwidthRange {
  min: number;
  max: number;
}

/** What each mode allows regardless of the device (min follows the channelizer's own floor). */
const MODE_RANGE: Partial<Record<VfoMode, BandwidthRange>> = {
  wfm: { min: 10_000, max: 1_000_000 },
  nfm: { min: 1_000, max: 1_000_000 },
  am: { min: 1_000, max: 500_000 },
  usb: { min: 300, max: 12_000 },
  lsb: { min: 300, max: 12_000 },
  cw: { min: 100, max: 4_000 },
};

/** The channel rate must exceed the filtered width by this factor (1.15 in the channelizer). */
const RATE_MARGIN = 1.2;

function isSideband(mode: VfoMode): boolean {
  return mode === "usb" || mode === "lsb";
}

/** Widest bandwidth the channelizer can realise at `sampleRateHz` (not capped by the mode). */
export function maxBandwidthForRate(mode: VfoMode, sampleRateHz: number): number {
  if (!(sampleRateHz > 0)) return Infinity;
  // SSB filters the whole sideband symmetrically around DC: 2 * (low cut + bandwidth).
  const raw = isSideband(mode)
    ? sampleRateHz / (2 * RATE_MARGIN) - SSB_LOW_CUT_HZ
    : sampleRateHz / RATE_MARGIN;
  return Math.max(0, Math.floor(raw / 100) * 100);
}

/** Allowed bandwidth range for a mode, narrowed by the device sample rate when known. */
export function bandwidthRange(mode: VfoMode, sampleRateHz?: number | null): BandwidthRange {
  const base = MODE_RANGE[mode] ?? bandwidthLimits(mode);
  if (!sampleRateHz) return base;
  const max = Math.min(base.max, maxBandwidthForRate(mode, sampleRateHz));
  return { min: base.min, max: Math.max(base.min, max) };
}

/** Drag/wheel snap grid: fine for narrow channels, coarser for wide ones. */
export function bandwidthSnap(hz: number): number {
  return hz < 20_000 ? 100 : hz < 100_000 ? 500 : 1_000;
}

export function snapBandwidth(hz: number): number {
  const step = bandwidthSnap(hz);
  return Math.round(hz / step) * step;
}

export function clampBandwidth(mode: VfoMode, hz: number, sampleRateHz?: number | null): number {
  const { min, max } = bandwidthRange(mode, sampleRateHz);
  return Math.min(max, Math.max(min, hz));
}

/**
 * Parse a typed bandwidth. An explicit unit (`Hz`, `kHz`, `k`) wins. A bare number is read as kHz
 * when that lands inside the mode's range and as Hz otherwise ("12.5" -> 12.5 kHz, "8500" -> 8.5
 * kHz, "500" in CW -> 500 Hz). Returns whole Hz, or null when the text is not a number.
 */
export function parseBandwidth(
  text: string,
  mode: VfoMode,
  sampleRateHz?: number | null,
): number | null {
  const m = /^\s*(\d+(?:[.,]\d+)?)\s*(khz|k|hz)?\s*$/i.exec(text);
  if (!m) return null;
  const n = Number((m[1] ?? "").replace(",", "."));
  if (!Number.isFinite(n)) return null;
  const unit = (m[2] ?? "").toLowerCase();
  if (unit === "hz") return Math.round(n);
  if (unit === "k" || unit === "khz") return Math.round(n * 1000);
  const { min, max } = bandwidthRange(mode, sampleRateHz);
  const asKhz = Math.round(n * 1000);
  const inRange = (hz: number) => hz >= min && hz <= max;
  if (inRange(asKhz) || !inRange(Math.round(n))) return asKhz;
  return Math.round(n);
}

export type BandwidthCheck = { ok: true; hz: number } | { ok: false; message: string };

/** Validate a bandwidth against the mode range and the device sample rate. */
export function checkBandwidth(
  mode: VfoMode,
  hz: number,
  sampleRateHz?: number | null,
): BandwidthCheck {
  if (!Number.isInteger(hz)) return { ok: false, message: "Enter a whole number of Hz." };
  const modeRange = MODE_RANGE[mode] ?? bandwidthLimits(mode);
  const { min, max } = bandwidthRange(mode, sampleRateHz);
  if (hz < min) {
    return { ok: false, message: `At least ${formatBandwidth(min)} in this mode.` };
  }
  if (hz > max) {
    return {
      ok: false,
      message:
        max < modeRange.max
          ? `At most ${formatBandwidth(max)} at this sample rate.`
          : `At most ${formatBandwidth(max)} in this mode.`,
    };
  }
  return { ok: true, hz };
}

/**
 * Bandwidth after `steps` wheel notches (positive = wider): about 8 % per notch, snapped, and
 * always at least one snap step so a single notch on a narrow channel still moves it.
 */
export function stepBandwidth(
  mode: VfoMode,
  current: number,
  steps: number,
  sampleRateHz?: number | null,
): number {
  if (steps === 0) return clampBandwidth(mode, current, sampleRateHz);
  let next = snapBandwidth(current * 1.08 ** steps);
  const floorMove = bandwidthSnap(current) * Math.sign(steps);
  if (Math.sign(next - current) !== Math.sign(steps)) next = current + floorMove;
  return clampBandwidth(mode, next, sampleRateHz);
}
