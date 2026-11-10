// Receiver range statistics (`GET /api/v1/adsb/range`): the farthest position heard per 5 degree
// bearing sector and altitude band, plus the message rate and aircraft count history. The
// numbers are collected by the ADS-B plugin and stored by the worker (ADR-0025); this file parses
// them and does the geometry for the polar plot and the coverage outline on the map.

import { isRec } from "../spectrum/api";

export const SECTORS = 72;
export const SECTOR_DEG = 360 / SECTORS;

/** Altitude bands in the order of the `band` index. */
export const BAND_LABELS: readonly string[] = [
  "ground to 5 000 ft",
  "5 000 to 10 000 ft",
  "10 000 to 20 000 ft",
  "20 000 to 30 000 ft",
  "30 000 to 40 000 ft",
  "above 40 000 ft",
];

export interface RangeCell {
  sector: number;
  band: number;
  km: number;
  /** ICAO address of the aircraft that set the record; may be empty. */
  icao: string;
}

export interface ActivityPoint {
  tsMs: number;
  msgsPerS: number;
  aircraft: number;
  withPosition: number;
}

export interface RangeData {
  device: string;
  /** Where the maxima were measured from; null until the plugin ran with a receiver position. */
  receiver: { lat: number; lon: number } | null;
  /** When the statistics began (first event at this position). */
  sinceMs: number | null;
  cells: RangeCell[];
  /** Seconds each point of `series` stands for. */
  bucketS: number;
  series: ActivityPoint[];
}

const num = (v: unknown): number | null => (typeof v === "number" && Number.isFinite(v) ? v : null);

export function parseRange(raw: unknown): RangeData | null {
  if (!isRec(raw) || raw.v !== 1) return null;
  const cells: RangeCell[] = [];
  for (const c of Array.isArray(raw.cells) ? raw.cells : []) {
    if (!Array.isArray(c)) continue;
    const [sector, band, km, icao] = c as unknown[];
    const s = num(sector);
    const b = num(band);
    const k = num(km);
    if (s === null || b === null || k === null) continue;
    if (!Number.isInteger(s) || s < 0 || s >= SECTORS) continue;
    if (!Number.isInteger(b) || b < 0 || b >= BAND_LABELS.length || k < 0) continue;
    cells.push({ sector: s, band: b, km: k, icao: typeof icao === "string" ? icao : "" });
  }
  const series: ActivityPoint[] = [];
  for (const p of Array.isArray(raw.series) ? raw.series : []) {
    if (!Array.isArray(p)) continue;
    const [ts, rate, aircraft, withPosition] = p as unknown[];
    const t = num(ts);
    if (t === null) continue;
    series.push({
      tsMs: t,
      msgsPerS: num(rate) ?? 0,
      aircraft: num(aircraft) ?? 0,
      withPosition: num(withPosition) ?? 0,
    });
  }
  const rx = isRec(raw.receiver) ? raw.receiver : null;
  const lat = rx ? num(rx.lat) : null;
  const lon = rx ? num(rx.lon) : null;
  return {
    device: typeof raw.device === "string" ? raw.device : "",
    receiver: lat !== null && lon !== null ? { lat, lon } : null,
    sinceMs: num(raw.since_ms),
    cells,
    bucketS: num(raw.bucket_s) ?? 60,
    series,
  };
}

/** Farthest distance per sector (km, 0 where nothing was heard); `band` null = all bands. */
export function sectorMax(cells: readonly RangeCell[], band: number | null): number[] {
  const out = new Array<number>(SECTORS).fill(0);
  for (const c of cells) {
    if (band !== null && c.band !== band) continue;
    if (c.km > (out[c.sector] ?? 0)) out[c.sector] = c.km;
  }
  return out;
}

/** Farthest distance per altitude band (km, 0 where nothing was heard). */
export function bandMax(cells: readonly RangeCell[]): number[] {
  const out = new Array<number>(BAND_LABELS.length).fill(0);
  for (const c of cells) {
    if (c.km > (out[c.band] ?? 0)) out[c.band] = c.km;
  }
  return out;
}

/** The single longest reception in the data. */
export function farthest(cells: readonly RangeCell[]): RangeCell | null {
  let best: RangeCell | null = null;
  for (const c of cells) if (best === null || c.km > best.km) best = c;
  return best;
}

/** Bearing in degrees (0 = north) at the middle of a sector. */
export const sectorCentre = (sector: number): number => (sector + 0.5) * SECTOR_DEG;

/** Compass name of a sector's centre ("NNE"). */
export function compass(sector: number): string {
  const names = [
    "N",
    "NNE",
    "NE",
    "ENE",
    "E",
    "ESE",
    "SE",
    "SSE",
    "S",
    "SSW",
    "SW",
    "WSW",
    "W",
    "WNW",
    "NW",
    "NNW",
  ];
  return names[Math.round(sectorCentre(sector) / 22.5) % 16] ?? "N";
}

const EARTH_KM = 6371;

/** The point `km` away from `from` along `bearingDeg` (great circle). */
export function destination(
  from: { lat: number; lon: number },
  bearingDeg: number,
  km: number,
): [number, number] {
  const d = km / EARTH_KM;
  const b = (bearingDeg * Math.PI) / 180;
  const lat1 = (from.lat * Math.PI) / 180;
  const lon1 = (from.lon * Math.PI) / 180;
  const lat2 = Math.asin(Math.sin(lat1) * Math.cos(d) + Math.cos(lat1) * Math.sin(d) * Math.cos(b));
  const lon2 =
    lon1 +
    Math.atan2(
      Math.sin(b) * Math.sin(d) * Math.cos(lat1),
      Math.cos(d) - Math.sin(lat1) * Math.sin(lat2),
    );
  const lon = (((lon2 * 180) / Math.PI + 540) % 360) - 180;
  return [(lat2 * 180) / Math.PI, lon];
}

/** Coverage outline for the map: one point per sector, at the sector's farthest reception. */
export function outline(
  receiver: { lat: number; lon: number },
  maxima: readonly number[],
): [number, number][] {
  return maxima.map((km, sector) =>
    km > 0
      ? destination(receiver, sectorCentre(sector), km)
      : ([receiver.lat, receiver.lon] as [number, number]),
  );
}

/** A round scale ring step (km) for a plot that reaches `maxKm`: 10, 25, 50, 100, 250, 500... */
export function ringStep(maxKm: number): number {
  const wanted = Math.max(maxKm, 1) / 4;
  for (const step of [5, 10, 25, 50, 100, 250, 500, 1000]) if (step >= wanted) return step;
  return 1000;
}
