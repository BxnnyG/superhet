import { describe, expect, it } from "vitest";
import { parseStatus, type Aircraft } from "./model";
import { adsbStatusRaw, aircraftRaw } from "./testing";
import {
  MAX_POINTS,
  MIN_STEP_MS,
  TRAIL_NEWEST,
  TRAIL_OLDEST,
  TrailStore,
  trailAlpha,
  trailSegments,
  type TrailPoint,
} from "./trails";
import { DEFAULT_PREFS, loadPrefs, PREFS_KEY, savePrefs } from "./prefs";

function planes(...raw: Record<string, unknown>[]): Aircraft[] {
  const status = parseStatus(adsbStatusRaw({ aircraft: raw.map((r) => aircraftRaw(r)) }));
  if (!status) throw new Error("fixture");
  return status.aircraft;
}

const at = (lat: number, extra: Record<string, unknown> = {}) =>
  planes({ lat, lon: 7, seen_pos_s: 0, ...extra });

describe("TrailStore", () => {
  it("collects positions when they change and ignores a repeated one", () => {
    const store = new TrailStore();
    store.record(at(51.0), 1_000_000);
    store.record(at(51.0), 1_003_000);
    store.record(at(51.1), 1_006_000);
    expect(store.get("3c6444").map((p) => p.lat)).toEqual([51.0, 51.1]);
  });

  it("keeps points at least one step apart", () => {
    const store = new TrailStore();
    store.record(at(51.0), 1_000_000);
    store.record(at(51.1), 1_000_000 + MIN_STEP_MS - 1);
    store.record(at(51.2), 1_000_000 + MIN_STEP_MS);
    expect(store.get("3c6444")).toHaveLength(2);
  });

  it("remembers the altitude and time of each point", () => {
    const store = new TrailStore();
    store.record(at(51.0, { alt_ft: 3000, seen_pos_s: 1.5 }), 1_000_000);
    expect(store.get("3c6444")[0]).toEqual({
      lat: 51.0,
      lon: 7,
      altFt: 3000,
      onGround: false,
      t: 998_500,
    });
  });

  it("skips aircraft without a position and forgets those that left the picture", () => {
    const store = new TrailStore();
    store.record(planes({ lat: undefined, lon: undefined }), 1_000_000);
    expect(store.get("3c6444")).toEqual([]);
    store.record(at(51.0), 1_000_000);
    expect(store.size).toBe(1);
    store.record(planes({ icao: "406a3d", lat: 50, lon: 7 }), 1_010_000);
    expect(store.get("3c6444")).toEqual([]);
    expect(store.size).toBe(1);
  });

  it("is bounded in length and age", () => {
    const store = new TrailStore();
    for (let i = 0; i < MAX_POINTS + 50; i++) {
      store.record(at(50 + i * 0.0001), 1_000_000 + i * MIN_STEP_MS);
    }
    expect(store.get("3c6444").length).toBeLessThanOrEqual(MAX_POINTS);
    store.clear();
    expect(store.size).toBe(0);
  });
});

describe("trailSegments", () => {
  const pts = (...ts: number[]): TrailPoint[] =>
    ts.map((t, i) => ({ lat: 50 + i, lon: 7, altFt: 1000 * i, onGround: false, t }));

  it("cuts the trail to the chosen length", () => {
    const now = 600_000;
    const points = pts(0, 200_000, 400_000, 590_000);
    // Last 5 minutes: only the two newest points are in.
    expect(trailSegments(points, now, 300_000)).toHaveLength(1);
    // Last 15 minutes: everything.
    expect(trailSegments(points, now, 900_000)).toHaveLength(3);
    // Switched off.
    expect(trailSegments(points, now, 0)).toEqual([]);
  });

  it("fades with age: newest strongest", () => {
    const now = 100_000;
    const segs = trailSegments(pts(40_000, 70_000, 100_000), now, 100_000);
    expect(segs).toHaveLength(2);
    const [older, newer] = segs;
    expect(newer?.strength).toBeGreaterThan(older?.strength ?? 1);
    expect(trailAlpha(1)).toBeCloseTo(TRAIL_NEWEST);
    expect(trailAlpha(0)).toBeCloseTo(TRAIL_OLDEST);
    expect(trailAlpha(2)).toBeCloseTo(TRAIL_NEWEST);
  });

  it("closes the gap to the live position", () => {
    const head: TrailPoint = { lat: 60, lon: 7, altFt: 9000, onGround: false, t: 100_000 };
    const segs = trailSegments(pts(90_000), 100_000, 300_000, head);
    expect(segs).toHaveLength(1);
    expect(segs[0]?.to).toBe(head);
  });
});

describe("map preferences", () => {
  it("default to five minutes of trail and the callsign label", () => {
    expect(DEFAULT_PREFS.trailMinutes).toBe(5);
    expect(DEFAULT_PREFS.labels).toEqual({ callsign: true, altitude: false, speed: false });
  });

  it("round-trip and fall back on garbage", () => {
    const store = new Map<string, string>();
    const prefs = {
      trailMinutes: 15 as const,
      labels: { callsign: false, altitude: true, speed: true },
      rings: true,
    };
    savePrefs(prefs, { setItem: (k, v) => void store.set(k, v) });
    expect(loadPrefs({ getItem: (k) => store.get(k) ?? null })).toEqual(prefs);
    expect(store.has(PREFS_KEY)).toBe(true);
    expect(loadPrefs({ getItem: () => "{nope" })).toEqual(DEFAULT_PREFS);
    expect(loadPrefs({ getItem: () => '{"trailMinutes":7,"labels":3}' })).toEqual(DEFAULT_PREFS);
  });
});
