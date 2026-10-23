import { describe, expect, it } from "vitest";
import {
  DEFAULT_STEP_HZ,
  bandwidthLimits,
  stepFrequency,
  SQUELCH_OFF_POS,
  defaultModeFor,
  hitTestVfo,
  meterFraction,
  passbandRange,
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

describe("passbandRange", () => {
  it("is centred for FM, AM and CW and one-sided for USB / LSB", () => {
    expect(passbandRange("nfm", 1000, 400)).toEqual({ loHz: 800, hiHz: 1200 });
    expect(passbandRange("cw", 1000, 200)).toEqual({ loHz: 900, hiHz: 1100 });
    // 300 Hz low cut, then the bandwidth.
    expect(passbandRange("usb", 10_000, 2_400)).toEqual({ loHz: 10_300, hiHz: 12_700 });
    expect(passbandRange("lsb", 10_000, 2_400)).toEqual({ loHz: 7_300, hiHz: 9_700 });
  });

  it("hit-tests a single-sideband VFO on its sideband and at its carrier", () => {
    const usb: Marker = { id: 1, freqHz: 10e6, bandwidthHz: 2_400, mode: "usb", paused: false };
    expect(hitTestVfo([usb], 10e6 + 1_500, 0)?.id).toBe(1);
    expect(hitTestVfo([usb], 10e6 - 1_500, 0)).toBeNull();
    expect(hitTestVfo([usb], 10e6 + 50, 100)?.id).toBe(1);
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

describe("keyboard and wheel stepping", () => {
  it("moves by whole steps on the step grid and never below one step", () => {
    expect(stepFrequency(145_500_000, 12_500, 1)).toBe(145_512_500);
    expect(stepFrequency(145_500_000, 12_500, -10)).toBe(145_375_000);
    expect(stepFrequency(100_003, 1_000, 1)).toBe(101_000);
    expect(stepFrequency(1_000, 1_000, -1)).toBe(1_000);
  });

  it("limits bandwidth to the ends of the mode presets", () => {
    expect(bandwidthLimits("nfm")).toEqual({ min: 6_250, max: 25_000 });
    expect(bandwidthLimits("wfm")).toEqual({ min: 100_000, max: 200_000 });
  });
});
