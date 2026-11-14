// FT8 / FT4 / WSPR plugin (org.superhet.decoder.ft8, docs/plugins-ft8.md): event names, the dial
// frequencies of the bands and the Maidenhead locator arithmetic. The decoded messages are plain
// plugin events (`ft8.message`, `wspr.spot`) in the decoder history; there is no live status
// document.

export const FT8_PLUGIN_ID = "org.superhet.decoder.ft8";
export const FT8_MESSAGE = "ft8.message";
export const WSPR_SPOT = "wspr.spot";

export type Ft8Mode = "ft8" | "ft4" | "wspr";

export interface DialPreset {
  /** Band name for the picker. */
  band: string;
  /** Dial frequency (the suppressed carrier, USB) in Hz. */
  dialHz: number;
}

const kHz = (v: number): number => Math.round(v * 1000);

// The usual calling frequencies, as WSJT-X lists them for "USB dial". HF bands need an
// upconverter or direct sampling in front of an RTL-SDR (docs/plugins-ft8.md).
export const FT8_PRESETS: readonly DialPreset[] = [
  { band: "160 m", dialHz: kHz(1840) },
  { band: "80 m", dialHz: kHz(3573) },
  { band: "60 m", dialHz: kHz(5357) },
  { band: "40 m", dialHz: kHz(7074) },
  { band: "30 m", dialHz: kHz(10136) },
  { band: "20 m", dialHz: kHz(14074) },
  { band: "17 m", dialHz: kHz(18100) },
  { band: "15 m", dialHz: kHz(21074) },
  { band: "12 m", dialHz: kHz(24915) },
  { band: "10 m", dialHz: kHz(28074) },
  { band: "6 m", dialHz: kHz(50313) },
  { band: "2 m", dialHz: kHz(144174) },
];

export const FT4_PRESETS: readonly DialPreset[] = [
  { band: "80 m", dialHz: kHz(3575) },
  { band: "40 m", dialHz: kHz(7047.5) },
  { band: "30 m", dialHz: kHz(10140) },
  { band: "20 m", dialHz: kHz(14080) },
  { band: "17 m", dialHz: kHz(18104) },
  { band: "15 m", dialHz: kHz(21140) },
  { band: "12 m", dialHz: kHz(24919) },
  { band: "10 m", dialHz: kHz(28180) },
  { band: "6 m", dialHz: kHz(50318) },
  { band: "2 m", dialHz: kHz(144170) },
];

export const WSPR_PRESETS: readonly DialPreset[] = [
  { band: "160 m", dialHz: kHz(1836.6) },
  { band: "80 m", dialHz: kHz(3568.6) },
  { band: "60 m", dialHz: kHz(5287.2) },
  { band: "40 m", dialHz: kHz(7038.6) },
  { band: "30 m", dialHz: kHz(10138.7) },
  { band: "20 m", dialHz: kHz(14095.6) },
  { band: "17 m", dialHz: kHz(18104.6) },
  { band: "15 m", dialHz: kHz(21094.6) },
  { band: "12 m", dialHz: kHz(24924.6) },
  { band: "10 m", dialHz: kHz(28124.6) },
  { band: "6 m", dialHz: kHz(50293) },
  { band: "4 m", dialHz: kHz(70091) },
  { band: "2 m", dialHz: kHz(144489) },
];

export const presetsOf = (mode: Ft8Mode): readonly DialPreset[] =>
  mode === "ft8" ? FT8_PRESETS : mode === "ft4" ? FT4_PRESETS : WSPR_PRESETS;

/** The preset whose dial frequency the VFO sits on (within 1 kHz of it), if any. */
export function presetAt(presets: readonly DialPreset[], freqHz: number): DialPreset | null {
  let best: DialPreset | null = null;
  for (const p of presets) {
    if (Math.abs(freqHz - p.dialHz) > 1000) continue;
    if (best === null || Math.abs(freqHz - p.dialHz) < Math.abs(freqHz - best.dialHz)) best = p;
  }
  return best;
}

/**
 * Centre of a 4 or 6 character Maidenhead locator as [latitude, longitude], or null when the text
 * is not a locator.
 */
export function gridCentre(grid: string): [number, number] | null {
  const g = grid.trim().toUpperCase();
  const m = /^([A-R]{2})(\d{2})(?:([A-X]{2}))?$/.exec(g);
  if (m === null) return null;
  const field = m[1] ?? "";
  const square = m[2] ?? "";
  const sub = m[3];
  const a = (c: string): number => c.charCodeAt(0) - 65;
  let lon = a(field.charAt(0)) * 20 - 180 + Number(square.charAt(0)) * 2;
  let lat = a(field.charAt(1)) * 10 - 90 + Number(square.charAt(1));
  if (sub === undefined) return [lat + 0.5, lon + 1];
  lon += (a(sub.charAt(0)) * 5) / 60;
  lat += (a(sub.charAt(1)) * 2.5) / 60;
  return [lat + 1.25 / 60, lon + 2.5 / 60];
}
