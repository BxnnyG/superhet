// AIS ships (org.superhet.decoder.ais, docs/plugins-ais.md): the vessels for the map and the list,
// built from the plugin's `ais.vessel` events (history from the database plus the live feed). One
// event carries everything the decoder knew about a vessel when it was written, so the newest
// event per MMSI is the picture and the older ones give the track. Pure; the map draws what comes out.

import type { DecoderEvent } from "../decoders/events";
import { eventData } from "../decoders/render";
import { AIS_PLUGIN_ID, AIS_VESSEL, KMH_PER_KN, formatSpeed } from "./format";

export { AIS_PLUGIN_ID, AIS_VESSEL, KMH_PER_KN, formatSpeed };

/** Positions kept per vessel for its track. */
export const MAX_TRACK = 60;
/** A vessel not heard for this long is drawn faded. */
export const STALE_AFTER_MS = 10 * 60_000;
/** A vessel not heard for this long is left off the map and the list. */
export const DROP_AFTER_MS = 3 * 3_600_000;

/** Kind of station: Class A / B transponder, shore base station, aid to navigation, SAR aircraft. */
export type VesselClass = "A" | "B" | "base" | "aton" | "sar";

/** What the symbol and colour show. */
export type VesselGroup =
  | "cargo"
  | "tanker"
  | "passenger"
  | "fishing"
  | "sailing"
  | "pleasure"
  | "tug"
  | "highspeed"
  | "special"
  | "other"
  | "base"
  | "aton"
  | "sar";

export const GROUP_LABEL: Record<VesselGroup, string> = {
  cargo: "Cargo",
  tanker: "Tanker",
  passenger: "Passenger",
  fishing: "Fishing",
  sailing: "Sailing",
  pleasure: "Pleasure craft",
  tug: "Tug / towing",
  highspeed: "High-speed craft",
  special: "Special craft (pilot, SAR, law enforcement, military ...)",
  other: "Other / unknown",
  base: "Base station",
  aton: "Aid to navigation",
  sar: "SAR aircraft",
};

export const GROUP_COLOR: Record<VesselGroup, string> = {
  cargo: "#34d399",
  tanker: "#f87171",
  passenger: "#60a5fa",
  fishing: "#fbbf24",
  sailing: "#c084fc",
  pleasure: "#f472b6",
  tug: "#fb923c",
  highspeed: "#22d3ee",
  special: "#e5e7eb",
  other: "#9ca3af",
  base: "#a78bfa",
  aton: "#facc15",
  sar: "#f43f5e",
};

export interface TrackPoint {
  lat: number;
  lon: number;
  tsMs: number;
}

export interface Vessel {
  mmsi: number;
  cls: VesselClass;
  name: string | null;
  callsign: string | null;
  imo: number | null;
  /** AIS ship and cargo type (20 to 99). */
  shipType: number | null;
  /** The decoder's text for the ship type, or the aid-to-navigation type. */
  typeText: string | null;
  status: number | null;
  statusText: string | null;
  lat: number | null;
  lon: number | null;
  sogKn: number | null;
  cogDeg: number | null;
  headingDeg: number | null;
  altitudeM: number | null;
  destination: string | null;
  eta: string | null;
  draughtM: number | null;
  lengthM: number | null;
  beamM: number | null;
  lastHeardMs: number;
  /** Events seen for this vessel (not messages: the decoder rations them). */
  reports: number;
  /** Oldest first, one point per distinct position. */
  track: TrackPoint[];
}

const str = (v: unknown): string | null => (typeof v === "string" && v !== "" ? v : null);
const num = (v: unknown): number | null => (typeof v === "number" && Number.isFinite(v) ? v : null);
const CLASSES: readonly VesselClass[] = ["A", "B", "base", "aton", "sar"];

/**
 * One entry per vessel heard. `events` may be in any order (the log is newest first, the live feed
 * prepends). Vessels without a position are listed too, they just have nothing to draw.
 */
export function vessels(events: readonly DecoderEvent[], nowMs: number = Date.now()): Vessel[] {
  const sorted = events
    .filter((e) => e.kind === AIS_VESSEL)
    .sort((a, b) => a.tsMs - b.tsMs || a.id - b.id);
  const byMmsi = new Map<number, Vessel>();
  for (const e of sorted) {
    const d = eventData(e);
    const mmsi = num(d.mmsi);
    if (mmsi === null || !Number.isInteger(mmsi) || mmsi <= 0) continue;
    const lat = num(d.lat);
    const lon = num(d.lon);
    const valid =
      lat !== null && lon !== null && lat >= -90 && lat <= 90 && lon >= -180 && lon <= 180;
    const cls = CLASSES.find((c) => c === d.class) ?? "A";
    let v = byMmsi.get(mmsi);
    if (v === undefined) {
      v = {
        mmsi,
        cls,
        name: null,
        callsign: null,
        imo: null,
        shipType: null,
        typeText: null,
        status: null,
        statusText: null,
        lat: null,
        lon: null,
        sogKn: null,
        cogDeg: null,
        headingDeg: null,
        altitudeM: null,
        destination: null,
        eta: null,
        draughtM: null,
        lengthM: null,
        beamM: null,
        lastHeardMs: e.tsMs,
        reports: 0,
        track: [],
      };
      byMmsi.set(mmsi, v);
    }
    v.reports += 1;
    v.cls = cls;
    v.lastHeardMs = e.tsMs;
    // Every event carries the merged picture, so a field the newest one lacks is unknown, but an
    // older event may have known it (the decoder restarted and forgot): keep what we had.
    v.name = str(d.name) ?? v.name;
    v.callsign = str(d.callsign) ?? v.callsign;
    v.imo = num(d.imo) ?? v.imo;
    v.shipType = num(cls === "aton" ? d.aton_type : d.ship_type) ?? v.shipType;
    v.typeText = str(cls === "aton" ? d.aton_type_text : d.ship_type_text) ?? v.typeText;
    v.status = num(d.status) ?? v.status;
    v.statusText = str(d.status_text) ?? v.statusText;
    v.destination = str(d.destination) ?? v.destination;
    v.eta = str(d.eta) ?? v.eta;
    v.draughtM = num(d.draught_m) ?? v.draughtM;
    v.lengthM = num(d.length_m) ?? v.lengthM;
    v.beamM = num(d.beam_m) ?? v.beamM;
    v.altitudeM = num(d.altitude_m) ?? v.altitudeM;
    if (valid) {
      v.lat = lat;
      v.lon = lon;
      v.sogKn = num(d.sog_kn);
      v.cogDeg = num(d.cog_deg);
      v.headingDeg = num(d.heading_deg);
      const last = v.track[v.track.length - 1];
      if (last === undefined || last.lat !== lat || last.lon !== lon) {
        v.track.push({ lat, lon, tsMs: e.tsMs });
        if (v.track.length > MAX_TRACK) v.track.shift();
      }
    }
  }
  return [...byMmsi.values()]
    .filter((v) => nowMs - v.lastHeardMs <= DROP_AFTER_MS)
    .sort((a, b) => b.lastHeardMs - a.lastHeardMs);
}

/** Vessels that have a position to draw. */
export const positioned = (list: readonly Vessel[]): (Vessel & { lat: number; lon: number })[] =>
  list.filter((v): v is Vessel & { lat: number; lon: number } => v.lat !== null && v.lon !== null);

/** What the symbol shows. */
export function groupOf(v: Pick<Vessel, "cls" | "shipType">): VesselGroup {
  if (v.cls === "base") return "base";
  if (v.cls === "aton") return "aton";
  if (v.cls === "sar") return "sar";
  const t = v.shipType;
  if (t === null) return "other";
  if (t === 30) return "fishing";
  if (t === 31 || t === 32 || t === 52) return "tug";
  if (t === 36) return "sailing";
  if (t === 37) return "pleasure";
  if (t === 35 || (t >= 50 && t <= 59)) return "special";
  if (t >= 40 && t <= 49) return "highspeed";
  if (t >= 60 && t <= 69) return "passenger";
  if (t >= 70 && t <= 79) return "cargo";
  if (t >= 80 && t <= 89) return "tanker";
  return "other";
}

/** At or below this speed a vessel is drawn as a dot, not as an arrow. */
export const MOORED_BELOW_KN = 0.5;

/** The direction the symbol points: the heading if the vessel sends one, else the course. */
export function pointing(v: Pick<Vessel, "headingDeg" | "cogDeg" | "sogKn">): number | null {
  if (v.sogKn !== null && v.sogKn < MOORED_BELOW_KN && v.headingDeg === null) return null;
  return v.headingDeg ?? v.cogDeg;
}

/** The name, else the callsign, else the MMSI. */
export function displayName(v: Pick<Vessel, "name" | "callsign" | "mmsi">): string {
  return v.name ?? v.callsign ?? String(v.mmsi);
}

/** "12 m x 4 m" */
export function formatSize(v: Pick<Vessel, "lengthM" | "beamM">): string | null {
  if (v.lengthM === null) return null;
  return v.beamM === null ? `${v.lengthM} m` : `${v.lengthM} m × ${v.beamM} m`;
}

/** 1 for a vessel heard recently, 0.45 once it is stale. */
export function vesselOpacity(v: Pick<Vessel, "lastHeardMs">, nowMs: number): number {
  return nowMs - v.lastHeardMs > STALE_AFTER_MS ? 0.45 : 1;
}

/** "3 min ago", "just now". */
export function heardAgo(v: Pick<Vessel, "lastHeardMs">, nowMs: number): string {
  const secs = Math.max(0, Math.round((nowMs - v.lastHeardMs) / 1000));
  if (secs < 10) return "just now";
  if (secs < 60) return `${secs} s ago`;
  const min = Math.floor(secs / 60);
  if (min < 60) return `${min} min ago`;
  const h = Math.floor(min / 60);
  return h < 24 ? `${h} h ${min % 60} min ago` : `${Math.floor(h / 24)} d ago`;
}

/** Case-insensitive match on name, callsign, MMSI, IMO and destination. */
export function matches(v: Vessel, query: string): boolean {
  const q = query.trim().toLowerCase();
  if (q === "") return true;
  return [v.name, v.callsign, String(v.mmsi), v.imo === null ? null : String(v.imo), v.destination]
    .filter((x): x is string => x !== null)
    .some((x) => x.toLowerCase().includes(q));
}
