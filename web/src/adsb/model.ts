// ADS-B plugin (org.superhet.decoder.adsb, docs/plugins-adsb.md): the live picture the plugin
// reports through `GET /api/v1/plugin-instances/{id}/status`, parsed defensively, plus the view
// helpers (age, fade-out, sorting, number formats).

import { INSTANCE_STATES, type InstanceState } from "../plugins/api";
import { ApiError, isRec } from "../spectrum/api";
import { bearingDeg, distanceKm } from "./geo";
import {
  formatAltFt,
  formatDistanceKm,
  formatSpeedKt,
  formatVrateFpm,
  type Measure,
  type UnitSystem,
} from "./units";

export const ADSB_PLUGIN_ID = "org.superhet.decoder.adsb";

/** The plugin drops aircraft it has not heard for this long; the view fades them out before. */
export const STALE_AFTER_S = 60;
/** Aircraft younger than this are drawn at full strength. */
export const FRESH_S = 15;
/** Opacity of an aircraft just before it disappears. */
const MIN_OPACITY = 0.25;

export interface Aircraft {
  /** Six lowercase hex digits (`~` prefix: non-ICAO address). */
  icao: string;
  callsign: string | null;
  /** Barometric altitude, feet. */
  altFt: number | null;
  onGround: boolean;
  speedKt: number | null;
  trackDeg: number | null;
  /** Climb rate, feet per minute (barometric, else geometric). */
  vrateFpm: number | null;
  /** Geometric (GNSS) altitude, feet. */
  altGeomFt: number | null;
  /** Indicated and true airspeed, knots. */
  iasKt: number | null;
  tasKt: number | null;
  mach: number | null;
  /** Where the nose points (true, else magnetic heading), degrees; differs from the track in wind. */
  headingDeg: number | null;
  lat: number | null;
  lon: number | null;
  /** Mean signal level, dBFS. */
  rssi: number | null;
  squawk: string | null;
  category: string | null;
  messages: number;
  /** Seconds since the last message when the plugin reported it. */
  seenS: number;
  /** Seconds since the last position when the plugin reported it. */
  seenPosS: number | null;
}

export interface AdsbStatus {
  mode: "starting" | "running" | "error";
  /** Receiver position the instance was configured with. */
  receiver: { lat: number; lon: number } | null;
  /** Messages decoded since the decoder started. */
  messages: number;
  messagesPerS: number | null;
  /** Aircraft in the live picture (more than listed when `truncated`). */
  total: number;
  withPosition: number;
  truncated: boolean;
  staleAfterS: number;
  error: string | null;
  aircraft: Aircraft[];
}

/** The reply of `GET /api/v1/plugin-instances/{id}/status`. */
export interface AdsbReply {
  pluginId: string;
  device: string | null;
  state: InstanceState;
  freqHz: number | null;
  /** Null until the plugin reported for the first time. */
  status: AdsbStatus | null;
  updatedMs: number;
}

const strOrNull = (v: unknown): string | null => (typeof v === "string" && v !== "" ? v : null);
const numOrNull = (v: unknown): number | null =>
  typeof v === "number" && Number.isFinite(v) ? v : null;

function parseAircraft(raw: unknown): Aircraft | null {
  if (!isRec(raw) || typeof raw.icao !== "string" || raw.icao === "") return null;
  const lat = numOrNull(raw.lat);
  const lon = numOrNull(raw.lon);
  const located = lat !== null && lon !== null && Math.abs(lat) <= 90 && Math.abs(lon) <= 180;
  return {
    icao: raw.icao.toLowerCase(),
    callsign: strOrNull(raw.callsign),
    altFt: numOrNull(raw.alt_ft),
    onGround: raw.on_ground === true,
    speedKt: numOrNull(raw.speed_kt),
    trackDeg: numOrNull(raw.track_deg),
    vrateFpm: numOrNull(raw.vrate_fpm) ?? numOrNull(raw.geom_rate_fpm),
    altGeomFt: numOrNull(raw.alt_geom_ft),
    iasKt: numOrNull(raw.ias_kt),
    tasKt: numOrNull(raw.tas_kt),
    mach: numOrNull(raw.mach),
    headingDeg: numOrNull(raw.heading_deg),
    lat: located ? lat : null,
    lon: located ? lon : null,
    rssi: numOrNull(raw.rssi),
    squawk: strOrNull(raw.squawk),
    category: strOrNull(raw.category),
    messages: numOrNull(raw.messages) ?? 0,
    seenS: numOrNull(raw.seen_s) ?? 0,
    seenPosS: numOrNull(raw.seen_pos_s),
  };
}

export function parseStatus(raw: unknown): AdsbStatus | null {
  if (!isRec(raw) || raw.v !== 1) return null;
  const rx = isRec(raw.receiver) ? raw.receiver : null;
  const rxLat = rx ? numOrNull(rx.lat) : null;
  const rxLon = rx ? numOrNull(rx.lon) : null;
  const mode = raw.mode === "running" || raw.mode === "error" ? raw.mode : "starting";
  const aircraft = (Array.isArray(raw.aircraft) ? raw.aircraft : [])
    .map(parseAircraft)
    .filter((a): a is Aircraft => a !== null);
  return {
    mode,
    receiver: rxLat !== null && rxLon !== null ? { lat: rxLat, lon: rxLon } : null,
    messages: numOrNull(raw.messages) ?? 0,
    messagesPerS: numOrNull(raw.messages_per_s),
    total: numOrNull(raw.total) ?? aircraft.length,
    withPosition: numOrNull(raw.with_position) ?? aircraft.filter((a) => a.lat !== null).length,
    truncated: raw.truncated === true,
    staleAfterS: numOrNull(raw.stale_after_s) ?? STALE_AFTER_S,
    error: strOrNull(raw.error),
    aircraft,
  };
}

export function parseReply(raw: unknown): AdsbReply {
  if (!isRec(raw) || typeof raw.plugin_id !== "string") throw new ApiError("invalid status reply");
  return {
    pluginId: raw.plugin_id,
    device: strOrNull(raw.device),
    state: INSTANCE_STATES.find((s) => s === raw.state) ?? "error",
    freqHz: numOrNull(raw.freq_hz),
    status: parseStatus(raw.status),
    updatedMs: numOrNull(raw.updated_unix_ms) ?? 0,
  };
}

/**
 * Seconds since the aircraft was last heard, now: what the plugin reported plus the time since
 * the status arrived (so the fade keeps going if a poll is late).
 */
export function ageOf(a: Aircraft, receivedAtMs: number, nowMs: number): number {
  return a.seenS + Math.max(0, nowMs - receivedAtMs) / 1000;
}

/** Opacity of an aircraft of this age: full while fresh, fading to a faint ghost at 60 s. */
export function fadeOf(ageS: number, staleAfterS: number = STALE_AFTER_S): number {
  if (ageS >= staleAfterS) return 0;
  if (ageS <= FRESH_S) return 1;
  const t = (ageS - FRESH_S) / (staleAfterS - FRESH_S);
  return Math.round((1 - t * (1 - MIN_OPACITY)) * 100) / 100;
}

/** An aircraft with the age it has at one moment, as the view uses it. */
export interface Tracked extends Aircraft {
  ageS: number;
  opacity: number;
  /** Great-circle distance from the receiver, km; null without a receiver position or a fix. */
  distKm: number | null;
  /** Bearing from the receiver to the aircraft, degrees. */
  bearing: number | null;
}

/** Aircraft that are still on the picture at `nowMs`, with age and opacity. */
export function tracked(status: AdsbStatus, receivedAtMs: number, nowMs: number): Tracked[] {
  const out: Tracked[] = [];
  for (const a of status.aircraft) {
    const ageS = ageOf(a, receivedAtMs, nowMs);
    const opacity = fadeOf(ageS, status.staleAfterS);
    if (opacity <= 0) continue;
    const from = status.receiver;
    const at = a.lat !== null && a.lon !== null ? { lat: a.lat, lon: a.lon } : null;
    out.push({
      ...a,
      ageS,
      opacity,
      distKm: from !== null && at !== null ? distanceKm(from, at) : null,
      bearing: from !== null && at !== null ? bearingDeg(from, at) : null,
    });
  }
  return out;
}

export const SORT_KEYS = [
  "callsign",
  "icao",
  "alt",
  "speed",
  "track",
  "distance",
  "rssi",
  "age",
] as const;
export type SortKey = (typeof SORT_KEYS)[number];
export type SortDir = "asc" | "desc";

const sortValue = (a: Tracked, key: SortKey): string | number | null => {
  switch (key) {
    case "callsign":
      return a.callsign?.toLowerCase() ?? null;
    case "icao":
      return a.icao;
    case "alt":
      return a.onGround ? 0 : a.altFt;
    case "speed":
      return a.speedKt;
    case "track":
      return a.trackDeg;
    case "distance":
      return a.distKm;
    case "rssi":
      return a.rssi;
    case "age":
      return a.ageS;
  }
};

/** Sorted copy; aircraft without a value always come last. */
export function sortAircraft(list: readonly Tracked[], key: SortKey, dir: SortDir): Tracked[] {
  const sign = dir === "asc" ? 1 : -1;
  return [...list].sort((a, b) => {
    const x = sortValue(a, key);
    const y = sortValue(b, key);
    if (x === null && y === null) return a.icao.localeCompare(b.icao);
    if (x === null) return 1;
    if (y === null) return -1;
    const order = typeof x === "string" && typeof y === "string" ? x.localeCompare(y) : +x - +y;
    return order === 0 ? a.icao.localeCompare(b.icao) : sign * order;
  });
}

/** Case-insensitive match on callsign, ICAO address or squawk. */
export function matchesFilter(a: Aircraft, query: string): boolean {
  const q = query.trim().toLowerCase();
  if (q === "") return true;
  return (
    a.icao.includes(q) ||
    (a.callsign?.toLowerCase().includes(q) ?? false) ||
    (a.squawk?.includes(q) ?? false)
  );
}

export const label = (a: Aircraft): string => a.callsign ?? a.icao;

/** Emergency squawk codes and what they mean (ICAO Doc 4444). */
export const EMERGENCY_SQUAWKS: Readonly<Record<string, string>> = {
  "7500": "Unlawful interference (hijack)",
  "7600": "Radio failure",
  "7700": "General emergency",
};

/** The meaning of an emergency squawk, or null for any other code. */
export const emergencyOf = (a: Pick<Aircraft, "squawk">): string | null =>
  a.squawk === null ? null : (EMERGENCY_SQUAWKS[a.squawk] ?? null);

const DASH: Measure = { text: "\u2013", other: "" };

export function formatAlt(a: Aircraft, u: UnitSystem): Measure {
  if (a.onGround) return { text: "ground", other: "" };
  return a.altFt === null ? DASH : formatAltFt(a.altFt, u);
}

export const formatGeomAlt = (a: Aircraft, u: UnitSystem): Measure =>
  a.altGeomFt === null ? DASH : formatAltFt(a.altGeomFt, u);

export const formatSpeed = (kt: number | null, u: UnitSystem): Measure =>
  kt === null ? DASH : formatSpeedKt(kt, u);

export const formatVrate = (a: Aircraft, u: UnitSystem): Measure =>
  a.vrateFpm === null ? DASH : formatVrateFpm(a.vrateFpm, u);

export const formatDistance = (km: number | null, u: UnitSystem): Measure =>
  km === null ? DASH : formatDistanceKm(km, u);

/** Arrow for the vertical rate: climbing, descending or level (under 100 ft/min). */
export function vrateArrow(fpm: number | null): "\u2191" | "\u2193" | "\u2192" | "" {
  if (fpm === null) return "";
  if (fpm >= 100) return "\u2191";
  if (fpm <= -100) return "\u2193";
  return "\u2192";
}

export const formatTrack = (deg: number | null): string =>
  deg === null ? "\u2013" : `${Math.round(deg) % 360}\u00b0`;

export const formatRssi = (a: Aircraft): string =>
  a.rssi === null ? "\u2013" : `${a.rssi.toFixed(1)} dB`;

export const formatMach = (mach: number | null): string =>
  mach === null ? "\u2013" : `M ${mach.toFixed(3).replace(/0$/, "")}`;

export function formatAge(ageS: number): string {
  return ageS < 1 ? "now" : `${Math.round(ageS)} s`;
}

/** The altitude bands of the map colours: upper limit in feet and colour. */
const BANDS: readonly (readonly [number, string])[] = [
  [5_000, "#fb923c"],
  [15_000, "#fbbf24"],
  [25_000, "#4ade80"],
  [35_000, "#38bdf8"],
];
const HIGH = "#818cf8";
const GROUND = "#a1a1aa";

/** Colour of an altitude for the map symbol and trail (low = warm, high = cool). */
export function altColor(altFt: number | null, onGround: boolean): string {
  if (onGround) return GROUND;
  if (altFt === null) return HIGH;
  for (const [limit, color] of BANDS) if (altFt < limit) return color;
  return HIGH;
}

export const altitudeColor = (a: Pick<Aircraft, "altFt" | "onGround">): string =>
  altColor(a.altFt, a.onGround);

/** Legend of [`altColor`] in the chosen unit. */
export function altitudeLegend(u: UnitSystem): readonly (readonly [string, string])[] {
  const names = BANDS.map(([limit]) => `< ${formatAltFt(limit, u).text}`);
  return [
    [GROUND, "ground"],
    ...BANDS.map(([, color], i) => [color, names[i] ?? ""] as const),
    [HIGH, "higher"],
  ];
}
