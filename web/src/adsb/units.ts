// Units for the ADS-B view. Everything the decoder reports is aviation units (feet, knots, feet per
// minute); the viewer picks what to read. The choice is kept in localStorage (this browser's
// profile, like the other view preferences) and applies to the table, labels, details, the range
// tab and the range rings. The value in the other unit goes into the tooltip.

export const UNIT_SYSTEMS = ["metric", "aviation", "mixed"] as const;
export type UnitSystem = (typeof UNIT_SYSTEMS)[number];

export const UNIT_SYSTEM_LABEL: Record<UnitSystem, string> = {
  metric: "Metric (km/h, m, km, m/s)",
  aviation: "Aviation (kt, ft, NM, ft/min)",
  mixed: "Mixed (km/h, ft, km, ft/min)",
};

export const UNITS_KEY = "superhet.adsb.units.v1";
export const DEFAULT_UNITS: UnitSystem = "metric";

export function loadUnits(storage: Pick<Storage, "getItem"> = window.localStorage): UnitSystem {
  try {
    const raw = storage.getItem(UNITS_KEY);
    return UNIT_SYSTEMS.find((u) => u === raw) ?? DEFAULT_UNITS;
  } catch {
    return DEFAULT_UNITS;
  }
}

export function saveUnits(
  units: UnitSystem,
  storage: Pick<Storage, "setItem"> = window.localStorage,
): void {
  try {
    storage.setItem(UNITS_KEY, units);
  } catch {
    // Not persisted; the choice still applies now.
  }
}

export const KT_PER_KMH = 1 / 1.852;
export const FT_PER_M = 1 / 0.3048;
export const KM_PER_NM = 1.852;

/** A value written in one unit, and the same value in the other system for the tooltip. */
export interface Measure {
  /** e.g. "835 km/h" */
  text: string;
  /** e.g. "451 kt"; the value in the other system. */
  other: string;
}

/** Which unit of a quantity a system uses: the metric one or the aviation one. */
type Side = "metric" | "aviation";
interface Sides {
  speed: Side;
  alt: Side;
  dist: Side;
  vrate: Side;
}
const SIDES: Record<UnitSystem, Sides> = {
  metric: { speed: "metric", alt: "metric", dist: "metric", vrate: "metric" },
  aviation: { speed: "aviation", alt: "aviation", dist: "aviation", vrate: "aviation" },
  mixed: { speed: "metric", alt: "aviation", dist: "metric", vrate: "aviation" },
};

const other = (s: Side): Side => (s === "metric" ? "aviation" : "metric");
const group = (n: number): string => Math.round(n).toLocaleString("en-US");

/** Speed in knots -> text. */
function speedIn(kt: number, side: Side): string {
  return side === "metric" ? `${Math.round(kt * 1.852)} km/h` : `${Math.round(kt)} kt`;
}
/** Altitude in feet -> text. */
function altIn(ft: number, side: Side): string {
  return side === "metric" ? `${group(ft * 0.3048)} m` : `${group(ft)} ft`;
}
/** Distance in kilometres -> text. */
function distIn(km: number, side: Side): string {
  const v = side === "metric" ? km : km / KM_PER_NM;
  const unit = side === "metric" ? "km" : "NM";
  return v >= 100 ? `${Math.round(v)} ${unit}` : `${v.toFixed(1)} ${unit}`;
}
/** Vertical rate in feet per minute -> text, signed. */
function vrateIn(fpm: number, side: Side): string {
  const sign = fpm > 0 ? "+" : fpm < 0 ? "−" : "";
  if (side === "metric") {
    const ms = Math.abs(fpm) * 0.00508;
    return `${sign}${ms.toFixed(1)} m/s`;
  }
  return `${sign}${group(Math.abs(fpm))} ft/min`;
}

export const formatSpeedKt = (kt: number, u: UnitSystem): Measure => ({
  text: speedIn(kt, SIDES[u].speed),
  other: speedIn(kt, other(SIDES[u].speed)),
});
export const formatAltFt = (ft: number, u: UnitSystem): Measure => ({
  text: altIn(ft, SIDES[u].alt),
  other: altIn(ft, other(SIDES[u].alt)),
});
export const formatDistanceKm = (km: number, u: UnitSystem): Measure => ({
  text: distIn(km, SIDES[u].dist),
  other: distIn(km, other(SIDES[u].dist)),
});
export const formatVrateFpm = (fpm: number, u: UnitSystem): Measure => ({
  text: vrateIn(fpm, SIDES[u].vrate),
  other: vrateIn(fpm, other(SIDES[u].vrate)),
});

/** Short unit names for column headers. */
export function unitNames(u: UnitSystem): { speed: string; alt: string; dist: string } {
  const s = SIDES[u];
  return {
    speed: s.speed === "metric" ? "km/h" : "kt",
    alt: s.alt === "metric" ? "m" : "ft",
    dist: s.dist === "metric" ? "km" : "NM",
  };
}

/** Whole kilometres per display unit of distance (a range ring step is a round number of these). */
export const kmPerDistanceUnit = (u: UnitSystem): number =>
  SIDES[u].dist === "metric" ? 1 : KM_PER_NM;
export const distanceUnitName = (u: UnitSystem): string => unitNames(u).dist;

/** Feet per display unit of altitude (for inputs written in the chosen unit). */
export const ftPerAltitudeUnit = (u: UnitSystem): number =>
  SIDES[u].alt === "metric" ? FT_PER_M : 1;

/** Altitude band edges of the range statistics (feet) written in the chosen unit. */
export function formatAltRange(fromFt: number | null, toFt: number | null, u: UnitSystem): string {
  const side = SIDES[u].alt;
  const part = (ft: number): string => group(side === "metric" ? roundNice(ft * 0.3048) : ft);
  const unit = side === "metric" ? "m" : "ft";
  if (fromFt === null && toFt !== null) return `ground to ${part(toFt)} ${unit}`;
  if (fromFt !== null && toFt === null) return `above ${part(fromFt)} ${unit}`;
  if (fromFt !== null && toFt !== null) return `${part(fromFt)} to ${part(toFt)} ${unit}`;
  return "all altitudes";
}

/** 1524 m -> 1500 m: band edges read better as round numbers. */
const roundNice = (m: number): number => Math.round(m / 100) * 100;
