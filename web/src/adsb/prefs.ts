// Map preferences kept in localStorage (this browser's profile, like the units): trail length,
// which label fields to draw, range rings.

export const TRAIL_MINUTES = [0, 1, 5, 15, 30] as const;
export type TrailMinutes = (typeof TRAIL_MINUTES)[number];

export interface LabelFields {
  callsign: boolean;
  altitude: boolean;
  speed: boolean;
}

export interface MapPrefs {
  /** How far back the trails reach, minutes; 0 = no trails. */
  trailMinutes: TrailMinutes;
  labels: LabelFields;
  /** Draw range rings around the receiver. */
  rings: boolean;
}

export const PREFS_KEY = "superhet.adsb.map.v1";
export const DEFAULT_PREFS: MapPrefs = {
  trailMinutes: 5,
  labels: { callsign: true, altitude: false, speed: false },
  rings: false,
};

export function loadPrefs(storage: Pick<Storage, "getItem"> = window.localStorage): MapPrefs {
  try {
    const raw: unknown = JSON.parse(storage.getItem(PREFS_KEY) ?? "null");
    if (typeof raw !== "object" || raw === null) return DEFAULT_PREFS;
    const o = raw as Record<string, unknown>;
    const l =
      typeof o.labels === "object" && o.labels !== null
        ? (o.labels as Record<string, unknown>)
        : {};
    const flag = (v: unknown, fallback: boolean): boolean =>
      typeof v === "boolean" ? v : fallback;
    return {
      trailMinutes: TRAIL_MINUTES.find((m) => m === o.trailMinutes) ?? DEFAULT_PREFS.trailMinutes,
      labels: {
        callsign: flag(l.callsign, DEFAULT_PREFS.labels.callsign),
        altitude: flag(l.altitude, DEFAULT_PREFS.labels.altitude),
        speed: flag(l.speed, DEFAULT_PREFS.labels.speed),
      },
      rings: flag(o.rings, DEFAULT_PREFS.rings),
    };
  } catch {
    return DEFAULT_PREFS;
  }
}

export function savePrefs(
  prefs: MapPrefs,
  storage: Pick<Storage, "setItem"> = window.localStorage,
): void {
  try {
    storage.setItem(PREFS_KEY, JSON.stringify(prefs));
  } catch {
    // Storage may be full or blocked; the preference is only a convenience.
  }
}
