// Where an aircraft is along the great circle between the origin and destination of its
// (database) route. The route comes from a callsign lookup and may be wrong: the aircraft can
// have been diverted, the flight number reused for another leg or the route changed. When the
// position does not fit the great circle at all, `plausible` is false and the panel says so.

import { bearingDeg, distanceKm, type Point } from "./geo";

const EARTH_KM = 6371.0088;
const rad = (deg: number): number => (deg * Math.PI) / 180;

export interface RouteProgress {
  /** Great-circle distance origin to destination, km. */
  totalKm: number;
  /** Distance flown along the route, km (0 before the origin, `totalKm` after the destination). */
  doneKm: number;
  /** Distance still to go, km. */
  remainingKm: number;
  /** 0 at the origin, 1 at the destination. */
  fraction: number;
  /** How far the aircraft is off the great circle, km (always positive). */
  offKm: number;
  /** The position is plausible for this route (near the great circle, between the airports). */
  plausible: boolean;
}

/** Off-route distance accepted as "on this route": a quarter of the way, at least 80 km. */
const tolerance = (totalKm: number): number => Math.max(80, 0.25 * totalKm);

/** Progress of `pos` along origin to destination; null when the airports are the same place. */
export function routeProgress(origin: Point, destination: Point, pos: Point): RouteProgress | null {
  const totalKm = distanceKm(origin, destination);
  if (totalKm < 1) return null;
  const d13 = distanceKm(origin, pos) / EARTH_KM;
  const dTheta = rad(bearingDeg(origin, pos) - bearingDeg(origin, destination));
  const cross = Math.asin(Math.max(-1, Math.min(1, Math.sin(d13) * Math.sin(dTheta))));
  const along = Math.acos(Math.max(-1, Math.min(1, Math.cos(d13) / Math.cos(cross))));
  // Behind the origin: the angle to the destination is more than 90 degrees.
  const signedKm = (Math.cos(dTheta) < 0 ? -along : along) * EARTH_KM;
  const offKm = Math.abs(cross) * EARTH_KM;
  const doneKm = Math.max(0, Math.min(totalKm, signedKm));
  const margin = 0.1 * totalKm;
  return {
    totalKm,
    doneKm,
    remainingKm: totalKm - doneKm,
    fraction: doneKm / totalKm,
    offKm,
    plausible: offKm <= tolerance(totalKm) && signedKm >= -margin && signedKm <= totalKm + margin,
  };
}

/** Minutes to cover `km` at `speedKt`; null when the speed is too low to mean anything. */
export function minutesAt(km: number, speedKt: number | null): number | null {
  if (speedKt === null || speedKt < 60) return null;
  return (km / (speedKt * 1.852)) * 60;
}

/** "1 h 25 min" / "42 min". */
export function formatDuration(minutes: number): string {
  const total = Math.max(0, Math.round(minutes));
  const h = Math.floor(total / 60);
  const m = total % 60;
  return h === 0 ? `${m} min` : `${h} h ${String(m).padStart(2, "0")} min`;
}
