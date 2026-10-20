// Pure helpers for the digit-tunable frequency readout (0000.000.000 = MHz.kHz.Hz).

export const DIGIT_COUNT = 10;
/** Index of the first kHz digit and the first Hz digit. */
export const KHZ_START = 4;
export const HZ_START = 7;
export const DEFAULT_MIN_HZ = 1;
export const DEFAULT_MAX_HZ = 9_999_999_999;

/** Value in Hz of one count of the digit at `index` (0 = most significant). */
export function placeValue(index: number): number {
  return 10 ** (DIGIT_COUNT - 1 - index);
}

/** The ten decimal digits of `hz` (clamped to 0..9 999 999 999), most significant first. */
export function digitsOf(hz: number): number[] {
  const n = Math.min(DEFAULT_MAX_HZ, Math.max(0, Math.round(hz)));
  return String(n).padStart(DIGIT_COUNT, "0").split("").map(Number);
}

export function clampHz(hz: number, min = DEFAULT_MIN_HZ, max = DEFAULT_MAX_HZ): number {
  return Math.min(max, Math.max(min, Math.round(hz)));
}

/** Add (dir = 1) or subtract (dir = -1) one count of the digit at `index`; carries and clamps. */
export function stepDigit(
  hz: number,
  index: number,
  dir: 1 | -1,
  min = DEFAULT_MIN_HZ,
  max = DEFAULT_MAX_HZ,
): number {
  return clampHz(hz + dir * placeValue(index), min, max);
}

/** Replace the digit at `index` by `digit` (0..9), keeping the others; clamps. */
export function setDigit(
  hz: number,
  index: number,
  digit: number,
  min = DEFAULT_MIN_HZ,
  max = DEFAULT_MAX_HZ,
): number {
  const place = placeValue(index);
  const current = Math.floor(Math.round(hz) / place) % 10;
  return clampHz(hz + (digit - current) * place, min, max);
}

/** Human name of a digit's place, e.g. "100 kHz". */
export function placeLabel(index: number): string {
  const p = placeValue(index);
  if (p >= 1e6) return `${p / 1e6} MHz`;
  if (p >= 1e3) return `${p / 1e3} kHz`;
  return `${p} Hz`;
}

/** Number of leading digits (MHz part only) that are insignificant zeros. */
export function leadingZeros(hz: number): number {
  const d = digitsOf(hz);
  let n = 0;
  while (n < KHZ_START - 1 && d[n] === 0) n++;
  return n;
}

/** Normalise wheel deltas of any deltaMode to pixels. */
export function wheelPixels(e: { deltaY: number; deltaMode: number }): number {
  return e.deltaMode === 1 ? e.deltaY * 33 : e.deltaMode === 2 ? e.deltaY * 400 : e.deltaY;
}

/** Wheel pixels that make up one tuning step (one notch of a typical mouse wheel). */
export const WHEEL_STEP_PX = 50;
