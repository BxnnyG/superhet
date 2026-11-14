// The AIS plugin's identity and the number formats shared by the map (model.ts) and the event log
// (decoders/render.ts). No imports, so both can use it without a cycle.

export const AIS_PLUGIN_ID = "org.superhet.decoder.ais";
export const AIS_VESSEL = "ais.vessel";

export const KMH_PER_KN = 1.852;

/** "12.3 kn · 22.8 km/h" */
export function formatSpeed(kn: number | null): string | null {
  if (kn === null) return null;
  return `${kn.toFixed(1)} kn · ${(kn * KMH_PER_KN).toFixed(1)} km/h`;
}
