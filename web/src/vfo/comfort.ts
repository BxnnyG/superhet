// HF comfort blocks of a VFO: noise blanker, manual notch, automatic notch and noise reduction
// (docs/api-devices.md). All off by default; strengths run 0..1. Pure helpers shared by the
// panel, the shortcuts and the VFO API.

import type { Vfo, VfoMode } from "./api";
import { SSB_LOW_CUT_HZ } from "./model";

export interface NotchSetting {
  /** Centre in Hz of the demodulated audio. */
  freqHz: number;
  /** -3 dB width in Hz. */
  widthHz: number;
}

export interface Comfort {
  /** Noise blanker strength. */
  nb?: number;
  notch?: NotchSetting;
  /** Automatic notch adaptation speed. */
  autoNotch?: number;
  /** Noise reduction strength. */
  nr?: number;
}

export type ComfortBlock = "nb" | "notch" | "autoNotch" | "nr";

export const NOTCH_FREQ_MIN_HZ = 50;
export const NOTCH_FREQ_MAX_HZ = 12_000;
export const NOTCH_WIDTH_MIN_HZ = 10;
export const NOTCH_WIDTH_MAX_HZ = 1_000;
const NOTCH_SNAP_HZ = 10;

/** Strength a block starts with when it is switched on. */
export const COMFORT_START = {
  nb: 0.5,
  autoNotch: 0.5,
  nr: 0.5,
  notch: { freqHz: 1_000, widthHz: 60 },
} as const;

export const COMFORT_LABEL: Record<ComfortBlock, string> = {
  nb: "Noise blanker",
  notch: "Notch",
  autoNotch: "Auto notch",
  nr: "Noise reduction",
};

/** The blocks work on demodulated audio of every mode but WFM (a multiplex, not audio). */
export function comfortSupported(mode: VfoMode): boolean {
  return mode !== "wfm";
}

export function isComfortOn(c: Comfort | null | undefined, block: ComfortBlock): boolean {
  return c?.[block] !== undefined;
}

export function comfortIsOff(c: Comfort | null | undefined): boolean {
  return !c || (c.nb === undefined && !c.notch && c.autoNotch === undefined && c.nr === undefined);
}

/** `c` with one block switched on (at its start value) or off. */
export function toggleComfort(c: Comfort | null | undefined, block: ComfortBlock): Comfort {
  const next: Comfort = { ...(c ?? {}) };
  if (isComfortOn(c, block)) delete next[block];
  else if (block === "notch") next.notch = { ...COMFORT_START.notch };
  else next[block] = COMFORT_START[block];
  return next;
}

export function clampStrength(v: number): number {
  return Number.isFinite(v) ? Math.min(1, Math.max(0, Math.round(v * 100) / 100)) : 0.5;
}

export function clampNotch(n: NotchSetting): NotchSetting {
  const snap = (v: number) => Math.round(v / NOTCH_SNAP_HZ) * NOTCH_SNAP_HZ;
  return {
    freqHz: Math.min(NOTCH_FREQ_MAX_HZ, Math.max(NOTCH_FREQ_MIN_HZ, snap(n.freqHz))),
    widthHz: Math.min(NOTCH_WIDTH_MAX_HZ, Math.max(NOTCH_WIDTH_MIN_HZ, snap(n.widthHz))),
  };
}

/** Audio frequencies (Hz) a VFO's passband covers: where the notch marker can sit usefully. */
export function audioRange(vfo: Pick<Vfo, "mode" | "bandwidthHz" | "cwPitchHz">): {
  loHz: number;
  hiHz: number;
} {
  const bw = vfo.bandwidthHz;
  switch (vfo.mode) {
    case "usb":
    case "lsb":
      return { loHz: SSB_LOW_CUT_HZ, hiHz: SSB_LOW_CUT_HZ + bw };
    case "cw": {
      const pitch = vfo.cwPitchHz ?? 700;
      return { loHz: Math.max(0, pitch - bw / 2), hiHz: pitch + bw / 2 };
    }
    default:
      return { loHz: 0, hiHz: bw / 2 };
  }
}

/** Parse the `comfort` member of a VFO document; null when absent, off or malformed. */
export function parseComfort(raw: unknown): Comfort | null {
  if (typeof raw !== "object" || raw === null) return null;
  const r = raw as Record<string, unknown>;
  const num = (v: unknown): number | undefined =>
    typeof v === "number" && Number.isFinite(v) ? v : undefined;
  const out: Comfort = {};
  const nb = num(r.nb);
  const auto = num(r.auto_notch);
  const nr = num(r.nr);
  if (nb !== undefined) out.nb = nb;
  if (auto !== undefined) out.autoNotch = auto;
  if (nr !== undefined) out.nr = nr;
  const n = r.notch;
  if (typeof n === "object" && n !== null) {
    const freq = num((n as Record<string, unknown>).freq_hz);
    const width = num((n as Record<string, unknown>).width_hz);
    if (freq !== undefined && width !== undefined) out.notch = { freqHz: freq, widthHz: width };
  }
  return comfortIsOff(out) ? null : out;
}

/** Wire form (`comfort` member of PATCH /vfos/{id}); `{}` switches everything off. */
export function comfortBody(c: Comfort): Record<string, unknown> {
  const body: Record<string, unknown> = {};
  if (c.nb !== undefined) body.nb = c.nb;
  if (c.notch) body.notch = { freq_hz: c.notch.freqHz, width_hz: c.notch.widthHz };
  if (c.autoNotch !== undefined) body.auto_notch = c.autoNotch;
  if (c.nr !== undefined) body.nr = c.nr;
  return body;
}
