// Condenses the per-VFO audio debug data into the few values the status bar shows (pure).

import type { EngineDebug } from "../audio/engine";

export interface StatusSummary {
  /** Buffer fill of the first VFO as "fill/target ms", or null. */
  buffer: string | null;
  /** Largest absolute drift correction in percent, signed, or null. */
  driftPct: number | null;
  /** Worst capture-to-speaker latency estimate in ms, or null. */
  latencyMs: number | null;
  underruns: number;
  overruns: number;
  /** Audio samples replaced by silence after lost frames (all VFOs). */
  lostSamples: number;
}

export function summarizeDebug(d: EngineDebug | null): StatusSummary {
  const empty: StatusSummary = {
    buffer: null,
    driftPct: null,
    latencyMs: null,
    underruns: 0,
    overruns: 0,
    lostSamples: 0,
  };
  if (!d || d.vfos.length === 0) return empty;
  const first = d.vfos[0];
  let drift: number | null = null;
  let latency: number | null = null;
  const out = { ...empty };
  for (const v of d.vfos) {
    if (drift === null || Math.abs(v.adj) > Math.abs(drift)) drift = v.adj;
    if (v.totalMs !== null && (latency === null || v.totalMs > latency)) latency = v.totalMs;
    out.underruns += v.underruns;
    out.overruns += v.overruns;
    out.lostSamples += v.gapSamples;
  }
  out.buffer = first ? `${first.fillMs.toFixed(0)}/${first.targetMs.toFixed(0)} ms` : null;
  out.driftPct = drift === null ? null : drift * 100;
  out.latencyMs = latency;
  return out;
}

export function formatDrift(pct: number | null): string {
  return pct === null ? "–" : `${pct >= 0 ? "+" : "−"}${Math.abs(pct).toFixed(3)} %`;
}
