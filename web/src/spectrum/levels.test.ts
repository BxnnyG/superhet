import { describe, expect, it } from "vitest";
import {
  DEFAULT_LEVEL_SETTINGS,
  LevelController,
  LevelEstimator,
  loadLevelSettings,
  saveLevelSettings,
  type DisplayRange,
  type LevelFrame,
} from "./levels";

const MIN_DB = -120;
const MAX_DB = 0;
const DB_PER_STEP = (MAX_DB - MIN_DB) / 255;

function dbToBin(db: number): number {
  return Math.round((db - MIN_DB) / DB_PER_STEP);
}

function frame(bins: Uint8Array): LevelFrame {
  return { bins, minDb: MIN_DB, maxDb: MAX_DB };
}

function noise(n: number, floorDb: number, seed = 1): Uint8Array {
  // Deterministic jitter of about ±2 dB around the floor.
  let s = seed;
  const out = new Uint8Array(n);
  for (let i = 0; i < n; i++) {
    s = (s * 1103515245 + 12345) & 0x7fffffff;
    out[i] = dbToBin(floorDb + ((s % 1000) / 1000 - 0.5) * 4);
  }
  return out;
}

describe("LevelEstimator", () => {
  it("places the window around flat noise", () => {
    const est = new LevelEstimator();
    const out: DisplayRange = { lowDb: 0, highDb: 0 };
    est.update(frame(new Uint8Array(1024).fill(dbToBin(-40))), 0, out);
    expect(out.lowDb).toBeCloseTo(-45, 0);
    expect(out.highDb).toBeCloseTo(0, 0); // floor + 40 = 0
    // Quieter floor: window follows, ceiling is floor + 40.
    const est2 = new LevelEstimator();
    est2.update(frame(new Uint8Array(1024).fill(dbToBin(-80))), 0, out);
    expect(out.lowDb).toBeCloseTo(-85, 0);
    expect(out.highDb).toBeCloseTo(-40, 0);
  });

  it("extends the ceiling over a strong peak but ignores a narrow one", () => {
    const out: DisplayRange = { lowDb: 0, highDb: 0 };
    const wide = noise(1000, -80);
    wide.fill(dbToBin(-10), 500, 520); // 2 % of bins
    new LevelEstimator().update(frame(wide), 0, out);
    expect(out.highDb).toBeCloseTo(-5, 0); // peak + 5
    expect(out.lowDb).toBeGreaterThan(-90);
    expect(out.lowDb).toBeLessThan(-80);

    const narrow = noise(1000, -80);
    narrow.fill(dbToBin(-10), 500, 502); // 0.2 % of bins stays below the 99.5th percentile
    new LevelEstimator().update(frame(narrow), 0, out);
    expect(out.highDb).toBeCloseTo(-40, -1); // floor + 40
  });

  it("converges to a new floor with ~1 s time constant", () => {
    const est = new LevelEstimator();
    const out: DisplayRange = { lowDb: 0, highDb: 0 };
    est.update(frame(new Uint8Array(512).fill(dbToBin(-90))), 0, out);
    const start = out.lowDb;
    const target = new Uint8Array(512).fill(dbToBin(-40));
    // One time constant: ~63 % of the way.
    for (let i = 0; i < 10; i++) est.update(frame(target), 0.1, out);
    const afterTau = (out.lowDb - start) / (-45 - start);
    expect(afterTau).toBeGreaterThan(0.55);
    expect(afterTau).toBeLessThan(0.7);
    for (let i = 0; i < 100; i++) est.update(frame(target), 0.1, out);
    expect(out.lowDb).toBeCloseTo(-45, 0);
  });

  it("clamps to the frame limits and keeps a minimum span", () => {
    const out: DisplayRange = { lowDb: 0, highDb: 0 };
    new LevelEstimator().update(frame(new Uint8Array(64).fill(255)), 0, out);
    expect(out.highDb).toBeLessThanOrEqual(MAX_DB);
    expect(out.highDb - out.lowDb).toBeGreaterThanOrEqual(5);
    new LevelEstimator().update(frame(new Uint8Array(64).fill(0)), 0, out);
    expect(out.lowDb).toBeGreaterThanOrEqual(MIN_DB);
  });
});

describe("LevelController", () => {
  it("follows frames in auto mode and freezes in manual mode", () => {
    const c = new LevelController();
    const bins = new Uint8Array(256).fill(dbToBin(-60));
    c.onFrame(frame(bins), 0);
    expect(c.range.lowDb).toBeCloseTo(-65, 0);
    let notified = 0;
    c.onChange(() => notified++);
    c.setManual(-100, -50);
    expect(c.auto).toBe(false);
    expect(notified).toBe(1);
    c.onFrame(frame(new Uint8Array(256).fill(dbToBin(-30))), 1000);
    expect(c.range).toEqual({ lowDb: -100, highDb: -50 });
    c.setAuto();
    expect(c.auto).toBe(true);
    c.onFrame(frame(new Uint8Array(256).fill(dbToBin(-30))), 2000);
    expect(c.range.lowDb).toBeCloseTo(-35, 0);
  });
});

describe("level settings persistence", () => {
  it("round-trips and falls back to defaults on bad data or failing storage", () => {
    const store = new Map<string, string>();
    const storage = {
      getItem: (k: string) => store.get(k) ?? null,
      setItem: (k: string, v: string) => void store.set(k, v),
    };
    expect(loadLevelSettings(storage)).toEqual(DEFAULT_LEVEL_SETTINGS);
    const s = { auto: false, manualLowDb: -90, manualHighDb: -30 };
    saveLevelSettings(s, storage);
    expect(loadLevelSettings(storage)).toEqual(s);
    store.set([...store.keys()][0] ?? "", "{not json");
    expect(loadLevelSettings(storage)).toEqual(DEFAULT_LEVEL_SETTINGS);
    const throwing = {
      getItem: () => {
        throw new Error("blocked");
      },
      setItem: () => {
        throw new Error("blocked");
      },
    };
    expect(loadLevelSettings(throwing)).toEqual(DEFAULT_LEVEL_SETTINGS);
    expect(() => saveLevelSettings(s, throwing)).not.toThrow();
  });
});
