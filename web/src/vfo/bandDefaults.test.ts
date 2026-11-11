import { describe, expect, it } from "vitest";
import { bandDefaultFor, retuneDefaults } from "./bandDefaults";
import { rasterForLocale } from "./mwRaster";
import { stepFor } from "./model";

describe("bandDefaultFor", () => {
  it("picks AM with the regional raster on medium wave", () => {
    expect(bandDefaultFor(1_000_000, 9_000)).toEqual({
      mode: "am",
      bandwidthHz: 6_000,
      stepHz: 9_000,
    });
    expect(bandDefaultFor(1_000_000, 10_000)?.stepHz).toBe(10_000);
    expect(bandDefaultFor(198_000, 10_000)?.stepHz).toBe(9_000); // long wave is 9 kHz everywhere
  });

  it("picks AM with 5 kHz steps on shortwave broadcast bands and time signals", () => {
    for (const hz of [6_100_000, 9_600_000, 15_300_000, 7_300_000, 10_000_000, 5_000_000]) {
      expect(bandDefaultFor(hz)).toEqual({ mode: "am", bandwidthHz: 6_000, stepHz: 5_000 });
    }
  });

  it("picks CW with 500 Hz and 100 Hz steps in the amateur CW segments", () => {
    for (const hz of [1_820_000, 3_550_000, 7_020_000, 10_110_000, 14_050_000, 28_020_000]) {
      expect(bandDefaultFor(hz)).toEqual({ mode: "cw", bandwidthHz: 500, stepHz: 100 });
    }
  });

  it("picks LSB below 10 MHz and USB from 10 MHz with 2.4 kHz", () => {
    expect(bandDefaultFor(3_750_000)).toEqual({ mode: "lsb", bandwidthHz: 2_400, stepHz: 100 });
    expect(bandDefaultFor(7_100_000)?.mode).toBe("lsb");
    expect(bandDefaultFor(14_250_000)).toEqual({ mode: "usb", bandwidthHz: 2_400, stepHz: 100 });
    expect(bandDefaultFor(28_500_000)?.mode).toBe("usb");
    // Outside the amateur bands (utility stations): same sideband rule, coarser step.
    expect(bandDefaultFor(4_625_000)).toEqual({ mode: "lsb", bandwidthHz: 2_400, stepHz: 1_000 });
    expect(bandDefaultFor(12_000_000 + 600_000)?.mode).toBe("usb");
  });

  it("treats CB as AM and ignores everything outside 100 kHz..30 MHz", () => {
    expect(bandDefaultFor(27_185_000)?.mode).toBe("am");
    expect(bandDefaultFor(50_000)).toBeNull();
    expect(bandDefaultFor(30_000_000)).toBeNull();
    expect(bandDefaultFor(145_500_000)).toBeNull();
    expect(bandDefaultFor(Number.NaN)).toBeNull();
  });
});

describe("retuneDefaults", () => {
  it("changes mode and bandwidth when the kind of band changes", () => {
    expect(retuneDefaults(1_000_000, 7_100_000, 9_000)).toEqual({
      mode: "lsb",
      bandwidthHz: 2_400,
    });
    expect(retuneDefaults(145_500_000, 14_200_000, 9_000)).toEqual({
      mode: "usb",
      bandwidthHz: 2_400,
    });
    expect(retuneDefaults(7_100_000, 7_020_000, 9_000)).toEqual({ mode: "cw", bandwidthHz: 500 });
  });

  it("does nothing inside one kind of band or outside the table", () => {
    expect(retuneDefaults(7_100_000, 7_150_000, 9_000)).toBeNull();
    expect(retuneDefaults(1_000_000, 1_300_000, 9_000)).toBeNull();
    expect(retuneDefaults(7_100_000, 145_500_000, 9_000)).toBeNull();
  });
});

describe("auto step", () => {
  it("follows the band while the VFO is in the band's mode, the mode default otherwise", () => {
    expect(stepFor("auto", "am", 6_100_000)).toBe(5_000);
    expect(stepFor("auto", "cw", 7_020_000)).toBe(100);
    expect(stepFor("auto", "lsb", 4_625_000)).toBe(1_000);
    expect(stepFor("auto", "nfm", 6_100_000)).toBe(12_500); // not the band's mode
    expect(stepFor("auto", "nfm")).toBe(12_500);
    expect(stepFor(500, "am", 6_100_000)).toBe(500); // an explicit step wins
  });
});

describe("mwRaster locale", () => {
  it("uses 10 kHz in the Americas and 9 kHz elsewhere", () => {
    expect(rasterForLocale("en-US")).toBe(10_000);
    expect(rasterForLocale("pt-BR")).toBe(10_000);
    expect(rasterForLocale("de-DE")).toBe(9_000);
    expect(rasterForLocale("en")).toBe(9_000);
    expect(rasterForLocale(undefined)).toBe(9_000);
  });
});
