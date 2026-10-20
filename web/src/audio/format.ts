// Text for the audio debug readout (pure).

import type { EngineDebug } from "./engine";

function signed(v: number, digits: number): string {
  return `${v >= 0 ? "+" : "−"}${Math.abs(v).toFixed(digits)}`;
}

/** One line per VFO: buffer fill, drift correction, counters and latency estimate. */
export function formatDebug(d: EngineDebug | null): string {
  if (!d) return "audio off";
  if (d.vfos.length === 0) return `ctx ${d.contextRate} Hz · no audio streams`;
  const lines = d.vfos.map((v) => {
    const lat =
      v.totalMs === null || v.transitMs === null
        ? "latency n/a"
        : `latency ≈ ${Math.round(v.totalMs)} ms (net ${Math.round(v.transitMs)} + buf ${Math.round(v.fillMs)} + out ${Math.round(d.outputMs)})`;
    return (
      `#${v.id} ${v.state} buf ${v.fillMs.toFixed(0)}/${v.targetMs.toFixed(0)} ms · ` +
      `drift ${signed(v.adj * 100, 3)} % · under ${v.underruns} over ${v.overruns} ` +
      `lost ${v.gapSamples} smp · ${lat}`
    );
  });
  return `ctx ${d.contextRate} Hz\n${lines.join("\n")}`;
}
