// APRS stations for the map, built from decoder events (history from the database plus the live
// feed): the last position and the track per station. Pure; the map draws what comes out.

import type { DecoderEvent } from "../decoders/events";
import { eventData } from "../decoders/render";

/** Positions kept per station for its track. */
export const MAX_TRACK = 60;
/** A station not heard for this long is drawn faded. */
export const STALE_AFTER_MS = 30 * 60_000;
/** A station not heard for this long is left off the map. */
export const DROP_AFTER_MS = 6 * 3_600_000;

export interface TrackPoint {
  lat: number;
  lon: number;
  tsMs: number;
}

export interface AprsStation {
  /** Callsign, or the object/item name. */
  id: string;
  /** Callsign of the sender when `id` is an object name. */
  via: string | null;
  lat: number;
  lon: number;
  /** Symbol table and code, "/>"; null if the packet had none. */
  symbol: string | null;
  comment: string | null;
  course: number | null;
  speedKn: number | null;
  altitudeM: number | null;
  path: string[];
  /** A killed object is not live any more. */
  live: boolean;
  lastHeardMs: number;
  packets: number;
  /** Oldest first, one point per distinct position. */
  track: TrackPoint[];
}

const str = (v: unknown): string | null => (typeof v === "string" && v !== "" ? v : null);
const num = (v: unknown): number | null => (typeof v === "number" && Number.isFinite(v) ? v : null);

/**
 * One entry per station that has sent a position. `events` may be in any order (the log is
 * newest first, the live feed prepends). Packets without a position update nothing.
 */
export function aprsStations(
  events: readonly DecoderEvent[],
  nowMs: number = Date.now(),
): AprsStation[] {
  const sorted = events
    .filter((e) => e.kind === "aprs.packet")
    .sort((a, b) => a.tsMs - b.tsMs || a.id - b.id);
  const byId = new Map<string, AprsStation>();
  for (const e of sorted) {
    const d = eventData(e);
    const lat = num(d.lat);
    const lon = num(d.lon);
    const source = str(d.source);
    if (lat === null || lon === null || source === null) continue;
    if (lat < -90 || lat > 90 || lon < -180 || lon > 180) continue;
    const name = str(d.name);
    const id = name ?? source;
    const path = Array.isArray(d.path)
      ? d.path.filter((x): x is string => typeof x === "string")
      : [];
    let s = byId.get(id);
    if (s === undefined) {
      s = {
        id,
        via: null,
        lat,
        lon,
        symbol: null,
        comment: null,
        course: null,
        speedKn: null,
        altitudeM: null,
        path,
        live: true,
        lastHeardMs: e.tsMs,
        packets: 0,
        track: [],
      };
      byId.set(id, s);
    }
    s.packets += 1;
    s.lat = lat;
    s.lon = lon;
    s.lastHeardMs = e.tsMs;
    s.path = path;
    s.via = name === null ? null : source;
    s.symbol = str(d.symbol) ?? s.symbol;
    s.comment = str(d.comment);
    s.course = num(d.course);
    s.speedKn = num(d.speed_kn);
    s.altitudeM = num(d.altitude_m);
    s.live = d.live !== false;
    const last = s.track[s.track.length - 1];
    if (last === undefined || last.lat !== lat || last.lon !== lon) {
      s.track.push({ lat, lon, tsMs: e.tsMs });
      if (s.track.length > MAX_TRACK) s.track.shift();
    }
  }
  return [...byId.values()]
    .filter((s) => nowMs - s.lastHeardMs <= DROP_AFTER_MS)
    .sort((a, b) => b.lastHeardMs - a.lastHeardMs);
}

/** 1 for a station heard recently, 0.45 once it is stale. */
export function stationOpacity(s: AprsStation, nowMs: number): number {
  return nowMs - s.lastHeardMs > STALE_AFTER_MS ? 0.45 : 1;
}

/** "3 min ago", "just now". */
export function heardAgo(s: AprsStation, nowMs: number): string {
  const secs = Math.max(0, Math.round((nowMs - s.lastHeardMs) / 1000));
  if (secs < 10) return "just now";
  if (secs < 60) return `${secs} s ago`;
  const min = Math.floor(secs / 60);
  if (min < 60) return `${min} min ago`;
  const h = Math.floor(min / 60);
  return h < 24 ? `${h} h ${min % 60} min ago` : `${Math.floor(h / 24)} d ago`;
}
