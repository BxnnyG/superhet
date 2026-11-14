// Great-circle distance and bearing from the receiver to an aircraft.

const EARTH_KM = 6371.0088;
const rad = (deg: number): number => (deg * Math.PI) / 180;

export interface Point {
  lat: number;
  lon: number;
}

/** Great-circle distance in kilometres (haversine). */
export function distanceKm(a: Point, b: Point): number {
  const dLat = rad(b.lat - a.lat);
  const dLon = rad(b.lon - a.lon);
  const h =
    Math.sin(dLat / 2) ** 2 + Math.cos(rad(a.lat)) * Math.cos(rad(b.lat)) * Math.sin(dLon / 2) ** 2;
  return 2 * EARTH_KM * Math.asin(Math.min(1, Math.sqrt(h)));
}

/** Initial bearing from `a` to `b` in degrees, 0 (north) to 360 exclusive. */
export function bearingDeg(a: Point, b: Point): number {
  const dLon = rad(b.lon - a.lon);
  const y = Math.sin(dLon) * Math.cos(rad(b.lat));
  const x =
    Math.cos(rad(a.lat)) * Math.sin(rad(b.lat)) -
    Math.sin(rad(a.lat)) * Math.cos(rad(b.lat)) * Math.cos(dLon);
  return ((Math.atan2(y, x) * 180) / Math.PI + 360) % 360;
}

const POINTS = [
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

/** 16-point compass name of a bearing, e.g. 123 -> "ESE". */
export function compassName(deg: number): string {
  return POINTS[Math.round((((deg % 360) + 360) % 360) / 22.5) % 16] ?? "N";
}
