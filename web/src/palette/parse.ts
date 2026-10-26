// Parser of the command palette's "tune" syntax: a frequency, optionally followed by a mode and a
// CTCSS tone, e.g. "433.92", "145.5m", "100,1 wfm", "7.1 lsb", "446.00625 nfm ctcss 88.5".
// Pure. Anything that is not a clean frequency command returns null and is searched as text.

import { CTCSS_TONES } from "../audio/tones";
import { parseFrequency } from "../spectrum/freq";
import { DEFAULT_MAX_HZ, DEFAULT_MIN_HZ } from "../ui/digits";
import { isVfoMode, type VfoMode } from "../vfo/api";

export interface TuneCommand {
  freqHz: number;
  /** Explicit mode, or undefined to keep / pick the usual one. */
  mode?: VfoMode;
  /** CTCSS tone squelch in Hz. */
  toneHz?: number;
}

/** Spoken aliases for the six modes. */
const MODE_ALIAS: Record<string, VfoMode> = {
  fm: "nfm",
  ssb: "usb",
  wbfm: "wfm",
  nbfm: "nfm",
  cw: "cw",
};

const UNIT = /^(hz|k|khz|m|mhz|g|ghz)$/;
const TONE_WORD = /^(ctcss|tone|pl)$/;

function modeOf(token: string): VfoMode | null {
  const t = token.toLowerCase();
  if (isVfoMode(t)) return t;
  return MODE_ALIAS[t] ?? null;
}

/** "88.5" or "88,5" -> a standard CTCSS tone, else null. */
function toneOf(token: string): number | null {
  const n = Number(token.replace(",", "."));
  return CTCSS_TONES.includes(n) ? n : null;
}

/**
 * Parse a tune command. The first token is the frequency (a separate unit token such as "145.5 m"
 * is joined to it); the rest may be a mode and `ctcss <Hz>` / `tone <Hz>` / `ctcss<Hz>` in any
 * order. A tone only makes sense for NFM and AM: it implies NFM when no mode is named and makes
 * the command invalid next to any other mode.
 */
export function parseTuneCommand(input: string): TuneCommand | null {
  const tokens = input
    .trim()
    .split(/\s+/)
    .filter((t) => t !== "");
  const first = tokens.shift();
  if (first === undefined || !/^[\d.,]/.test(first)) return null;
  let freqText = first;
  const next = tokens[0];
  if (next !== undefined && UNIT.test(next.toLowerCase())) {
    freqText += next;
    tokens.shift();
  }
  const freqHz = parseFrequency(freqText);
  if (freqHz === null || freqHz < DEFAULT_MIN_HZ || freqHz > DEFAULT_MAX_HZ) return null;

  let mode: VfoMode | undefined;
  let toneHz: number | undefined;
  while (tokens.length > 0) {
    const t = (tokens.shift() ?? "").toLowerCase();
    const asMode = modeOf(t);
    if (asMode !== null) {
      if (mode !== undefined && mode !== asMode) return null;
      mode = asMode;
      continue;
    }
    const glued = /^(ctcss|tone|pl)([\d.,]+)$/.exec(t);
    let toneToken: string | undefined;
    if (glued) {
      toneToken = glued[2];
    } else if (TONE_WORD.test(t)) {
      toneToken = tokens.shift();
    } else {
      return null;
    }
    const tone = toneToken === undefined ? null : toneOf(toneToken);
    if (tone === null || (toneHz !== undefined && toneHz !== tone)) return null;
    toneHz = tone;
  }
  if (toneHz !== undefined) {
    if (mode === undefined) mode = "nfm";
    else if (mode !== "nfm" && mode !== "am") return null;
  }
  return {
    freqHz,
    ...(mode !== undefined ? { mode } : {}),
    ...(toneHz !== undefined ? { toneHz } : {}),
  };
}
