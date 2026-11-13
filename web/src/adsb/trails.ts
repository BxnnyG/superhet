// Trails: where each aircraft has been, kept in the browser for as long as it stays on the
// picture (the plugin only reports the current position). Each point remembers its altitude so
// the trail can be coloured like the symbol, and its time so it can fade and be cut to the
// length the viewer chose.

import type { Aircraft } from "./model";

export interface TrailPoint {
  lat: number;
  lon: number;
  altFt: number | null;
  onGround: boolean;
  /** When the position was reported, ms since epoch. */
  t: number;
}

/** Two points of one aircraft are at least this far apart in time (a trail of 30 minutes stays small). */
export const MIN_STEP_MS = 2000;
/** Longest trail ever kept: 30 minutes at one point per step, with some slack. */
export const MAX_POINTS = 1000;
/** Points older than this are dropped whatever the setting is. */
export const MAX_AGE_MS = 31 * 60_000;

export class TrailStore {
  private readonly trails = new Map<string, TrailPoint[]>();

  /**
   * Take one status picture: appends each aircraft's position when it changed, and forgets
   * aircraft that are no longer listed.
   */
  record(aircraft: readonly Aircraft[], receivedAtMs: number): void {
    const listed = new Set<string>();
    for (const a of aircraft) {
      listed.add(a.icao);
      if (a.lat === null || a.lon === null) continue;
      const t = receivedAtMs - (a.seenPosS ?? a.seenS) * 1000;
      let points = this.trails.get(a.icao);
      if (points === undefined) {
        points = [];
        this.trails.set(a.icao, points);
      }
      const last = points[points.length - 1];
      if (
        last !== undefined &&
        (t - last.t < MIN_STEP_MS || (last.lat === a.lat && last.lon === a.lon))
      ) {
        continue;
      }
      points.push({ lat: a.lat, lon: a.lon, altFt: a.altFt, onGround: a.onGround, t });
      if (points.length > MAX_POINTS) points.splice(0, points.length - MAX_POINTS);
      while (points[0] !== undefined && t - points[0].t > MAX_AGE_MS) points.shift();
    }
    for (const icao of this.trails.keys()) if (!listed.has(icao)) this.trails.delete(icao);
  }

  get(icao: string): readonly TrailPoint[] {
    return this.trails.get(icao) ?? [];
  }

  clear(): void {
    this.trails.clear();
  }

  get size(): number {
    return this.trails.size;
  }
}

/** A piece of a trail between two recorded points, with how strongly to draw it. */
export interface TrailSegment {
  from: TrailPoint;
  to: TrailPoint;
  /** 1 = just now, towards 0 at the end of the window. */
  strength: number;
}

/** Opacity of the newest and the oldest end of a trail. */
export const TRAIL_NEWEST = 0.85;
export const TRAIL_OLDEST = 0.1;

/**
 * The segments of `points` that fall inside the last `windowMs`, newest strongest. `head` (the
 * aircraft's live position) closes the gap to the last recorded point.
 */
export function trailSegments(
  points: readonly TrailPoint[],
  nowMs: number,
  windowMs: number,
  head?: TrailPoint,
): TrailSegment[] {
  if (windowMs <= 0) return [];
  const inWindow = points.filter((p) => nowMs - p.t <= windowMs);
  if (head !== undefined) inWindow.push(head);
  const out: TrailSegment[] = [];
  for (let i = 1; i < inWindow.length; i++) {
    const from = inWindow[i - 1];
    const to = inWindow[i];
    if (from === undefined || to === undefined) continue;
    const age = Math.min(1, Math.max(0, (nowMs - to.t) / windowMs));
    out.push({ from, to, strength: 1 - age });
  }
  return out;
}

/** Canvas alpha of a segment of strength `s` (0..1). */
export const trailAlpha = (strength: number): number =>
  TRAIL_OLDEST + (TRAIL_NEWEST - TRAIL_OLDEST) * Math.min(1, Math.max(0, strength));
