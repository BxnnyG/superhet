import { describe, expect, it } from "vitest";
import {
  bandwidthRange,
  checkBandwidth,
  maxBandwidthForRate,
  parseBandwidth,
  snapBandwidth,
  stepBandwidth,
} from "./bandwidth";

describe("range", () => {
  it("is much wider than the presets", () => {
    expect(bandwidthRange("wfm")).toEqual({ min: 10_000, max: 1_000_000 });
    expect(bandwidthRange("nfm")).toEqual({ min: 1_000, max: 1_000_000 });
    expect(bandwidthRange("cw")).toEqual({ min: 100, max: 4_000 });
  });

  it("the device sample rate narrows the top end", () => {
    // 250 kS/s: the channel rate must clear 1.2x the filtered width.
    expect(maxBandwidthForRate("nfm", 250_000)).toBe(208_300);
    expect(bandwidthRange("nfm", 250_000).max).toBe(208_300);
    expect(bandwidthRange("nfm", 2_400_000).max).toBe(1_000_000);
    // SSB filters the whole sideband around DC (twice low cut + bandwidth).
    expect(maxBandwidthForRate("usb", 24_000)).toBe(9_700);
    expect(bandwidthRange("usb", 2_400_000).max).toBe(12_000);
  });

  it("never drops below the mode minimum", () => {
    expect(bandwidthRange("usb", 1_000).max).toBe(300);
  });
});

describe("parse", () => {
  it("honours explicit units", () => {
    expect(parseBandwidth("12.5 kHz", "nfm")).toBe(12_500);
    expect(parseBandwidth("12,5k", "nfm")).toBe(12_500);
    expect(parseBandwidth("8500 Hz", "nfm")).toBe(8_500);
    expect(parseBandwidth("150KHZ", "wfm")).toBe(150_000);
  });

  it("reads a bare number as kHz when that fits the mode, else as Hz", () => {
    expect(parseBandwidth("12.5", "nfm")).toBe(12_500);
    expect(parseBandwidth("150", "wfm")).toBe(150_000);
    expect(parseBandwidth("8500", "nfm")).toBe(8_500);
    expect(parseBandwidth("2400", "usb")).toBe(2_400);
    expect(parseBandwidth("2.4", "usb")).toBe(2_400);
    expect(parseBandwidth("500", "cw")).toBe(500);
  });

  it("rejects text that is not a width", () => {
    expect(parseBandwidth("", "nfm")).toBeNull();
    expect(parseBandwidth("wide", "nfm")).toBeNull();
    expect(parseBandwidth("-5", "nfm")).toBeNull();
    expect(parseBandwidth("12 MHz", "nfm")).toBeNull();
  });
});

describe("check", () => {
  it("accepts any whole Hz inside the range", () => {
    expect(checkBandwidth("nfm", 13_337)).toEqual({ ok: true, hz: 13_337 });
    expect(checkBandwidth("wfm", 250_000, 2_400_000)).toEqual({ ok: true, hz: 250_000 });
  });

  it("names the limit that is violated", () => {
    expect(checkBandwidth("nfm", 500)).toEqual({
      ok: false,
      message: "At least 1 kHz in this mode.",
    });
    expect(checkBandwidth("usb", 20_000)).toEqual({
      ok: false,
      message: "At most 12 kHz in this mode.",
    });
    expect(checkBandwidth("nfm", 300_000, 250_000)).toEqual({
      ok: false,
      message: "At most 208.3 kHz at this sample rate.",
    });
    expect(checkBandwidth("nfm", 1_000.5).ok).toBe(false);
  });
});

describe("snap and wheel stepping", () => {
  it("snaps to 100 Hz, 500 Hz and 1 kHz by width", () => {
    expect(snapBandwidth(12_449)).toBe(12_400);
    expect(snapBandwidth(34_800)).toBe(35_000);
    expect(snapBandwidth(154_300)).toBe(154_000);
  });

  it("one notch moves a narrow channel by at least one grid step", () => {
    expect(stepBandwidth("cw", 500, 1)).toBe(500 + 100);
    expect(stepBandwidth("cw", 500, -1)).toBe(400);
    expect(stepBandwidth("nfm", 12_500, 1)).toBe(13_500);
    expect(stepBandwidth("nfm", 12_500, -1)).toBe(11_600);
  });

  it("larger step counts scale and the result is clamped", () => {
    expect(stepBandwidth("nfm", 12_500, 20)).toBe(58_500);
    expect(stepBandwidth("usb", 2_400, 40)).toBe(12_000);
    expect(stepBandwidth("nfm", 1_100, -10)).toBe(1_000);
    expect(stepBandwidth("nfm", 200_000, 30, 250_000)).toBe(208_300);
    expect(stepBandwidth("nfm", 12_500, 0)).toBe(12_500);
  });
});
