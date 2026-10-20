import { describe, expect, it } from "vitest";
import {
  DEFAULT_STEP_HZ,
  SQUELCH_OFF_POS,
  defaultModeFor,
  hitTestVfo,
  meterFraction,
  sliderToSquelch,
  snapFrequency,
  squelchToSlider,
  stepFor,
  type Marker,
} from "./model";

const m = (id: number, freqHz: number, bandwidthHz: number): Marker => ({
  id,
  freqHz,
  bandwidthHz,
  mode: "nfm",
  paused: false,
});

describe("snapping", () => {
  it("snaps to the nearest multiple of the step", () => {
    expect(snapFrequency(100_004_999, 10_000)).toBe(100_000_000);
    expect(snapFrequency(100_005_000, 10_000)).toBe(100_010_000);
    expect(snapFrequency(145_506_300, 12_500)).toBe(145_512_500);
    expect(snapFrequency(1_234_567, 100)).toBe(1_234_600);
  });

  it("never returns less than one step", () => {
    expect(snapFrequency(10, 1000)).toBe(1000);
    expect(snapFrequency(-5, 1000)).toBe(1000);
  });

  it("falls back to rounding for an invalid step", () => {
    expect(snapFrequency(1234.6, 0)).toBe(1235);
  });

  it("uses mode defaults for 'auto'", () => {
    expect(stepFor("auto", "wfm")).toBe(10_000);
    expect(stepFor("auto", "nfm")).toBe(12_500);
    expect(stepFor("auto", "am")).toBe(5_000);
    expect(stepFor(1000, "wfm")).toBe(1000);
    expect(DEFAULT_STEP_HZ.wfm).toBe(10_000);
  });
});

describe("defaultModeFor", () => {
  it("picks WFM for FM broadcast, AM for airband, NFM otherwise", () => {
    expect(defaultModeFor(100e6)).toBe("wfm");
    expect(defaultModeFor(121.5e6)).toBe("am");
    expect(defaultModeFor(145.5e6)).toBe("nfm");
    expect(defaultModeFor(433.9e6)).toBe("nfm");
  });
});

describe("squelch slider", () => {
  it("maps off, range and clamps", () => {
    expect(squelchToSlider(null)).toBe(SQUELCH_OFF_POS);
    expect(sliderToSquelch(SQUELCH_OFF_POS)).toBeNull();
    expect(sliderToSquelch(-120)).toBe(-120);
    expect(sliderToSquelch(0)).toBe(0);
    expect(squelchToSlider(-200)).toBe(-120);
    expect(squelchToSlider(-47.4)).toBe(-47);
    expect(sliderToSquelch(squelchToSlider(-60))).toBe(-60);
  });
});

describe("meterFraction", () => {
  it("maps -120..0 dBFS to 0..1", () => {
    expect(meterFraction(-120)).toBe(0);
    expect(meterFraction(-60)).toBeCloseTo(0.5);
    expect(meterFraction(0)).toBe(1);
    expect(meterFraction(10)).toBe(1);
    expect(meterFraction(null)).toBe(0);
    expect(meterFraction(NaN)).toBe(0);
  });
});

describe("hitTestVfo", () => {
  const markers = [m(1, 100e6, 12_500), m(2, 100.005e6, 12_500), m(3, 101e6, 1000)];

  it("hits inside the bandwidth, nearest centre wins", () => {
    expect(hitTestVfo(markers, 100.001e6, 0)?.id).toBe(1);
    expect(hitTestVfo(markers, 100.004e6, 0)?.id).toBe(2);
  });

  it("uses the grab slop for narrow VFOs", () => {
    expect(hitTestVfo(markers, 101.0015e6, 0)).toBeNull();
    expect(hitTestVfo(markers, 101.0015e6, 2000)?.id).toBe(3);
  });

  it("returns null on empty spectrum", () => {
    expect(hitTestVfo(markers, 99e6, 100)).toBeNull();
    expect(hitTestVfo([], 99e6, 100)).toBeNull();
  });
});
