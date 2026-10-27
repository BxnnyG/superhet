// The shipped channel data against the published tables (docs/channel-plans.md lists the sources),
// and against the overlay's own copy in bandplan/data.ts so the two cannot drift apart.

import { describe, expect, it } from "vitest";
import { CHANNEL_PLANS } from "../bandplan/data";
import { expandPlan } from "../bandplan/model";
import { builtinPlans } from "./catalog";
import { rasterCount, rasterFreq, rasterName } from "./lookup";
import type { Plan } from "./schema";

const plan = (id: string): Plan => {
  const p = builtinPlans().find((x) => x.id === id);
  if (!p) throw new Error(`no plan ${id}`);
  return p;
};
const freq = (id: string, number: string) =>
  plan(id).channels.find((c) => c.number === number)?.freqHz;

describe("PMR446", () => {
  it("has 16 analogue channels from 446.00625 MHz in 12.5 kHz steps", () => {
    const p = plan("pmr446");
    expect(p.channels).toHaveLength(16);
    expect(freq("pmr446", "1")).toBe(446_006_250);
    expect(freq("pmr446", "8")).toBe(446_093_750);
    expect(freq("pmr446", "16")).toBe(446_193_750);
    expect(p.note).toMatch(/dPMR/);
    expect(p.licence).toBe("licence-free");
  });
});

describe("Freenet", () => {
  it("has the six German channels", () => {
    expect(plan("freenet-de").channels.map((c) => c.freqHz)).toEqual([
      149_025_000, 149_037_500, 149_050_000, 149_087_500, 149_100_000, 149_112_500,
    ]);
    expect(plan("freenet-de").regions).toEqual(["DE"]);
  });
});

describe("LPD433", () => {
  it("has 69 channels of 25 kHz", () => {
    const p = plan("lpd433");
    expect(p.channels).toHaveLength(69);
    expect(freq("lpd433", "1")).toBe(433_075_000);
    expect(freq("lpd433", "69")).toBe(434_775_000);
  });
});

describe("marine VHF", () => {
  it("names the key channels", () => {
    expect(freq("marine-vhf", "16")).toBe(156_800_000);
    expect(freq("marine-vhf", "70")).toBe(156_525_000);
    expect(freq("marine-vhf", "06")).toBe(156_300_000);
    expect(freq("marine-vhf", "13")).toBe(156_650_000);
    expect(plan("marine-vhf").channels.find((c) => c.number === "16")?.name).toMatch(/distress/i);
  });

  it("coast stations of the duplex channels transmit 4.6 MHz above the ship side", () => {
    const coast = plan("marine-vhf-coast");
    const duplex = coast.channels.filter((c) => !c.number.endsWith("B"));
    expect(duplex).toHaveLength(33);
    for (const c of duplex) {
      const ship = freq("marine-vhf", c.number);
      expect(ship, c.number).toBeDefined();
      expect(c.freqHz - (ship ?? 0), c.number).toBe(4_600_000);
    }
    expect(freq("marine-vhf-coast", "87B")).toBe(161_975_000);
    expect(freq("marine-vhf-coast", "88B")).toBe(162_025_000);
  });

  it("covers every international channel once, on the 25 kHz raster", () => {
    const ship = plan("marine-vhf").channels;
    const numbers = ship.map((c) => Number(c.number));
    // 1-28, 60-88 (the table has no channels 29-59).
    expect(numbers).toEqual([
      ...Array.from({ length: 28 }, (_, i) => i + 1),
      ...Array.from({ length: 29 }, (_, i) => i + 60),
    ]);
    for (const c of ship) expect((c.freqHz - 156_025_000) % 25_000, c.number).toBe(0);
  });
});

describe("CB radio", () => {
  it("has the 40-channel table, out of frequency order around 23-25", () => {
    const p = plan("cb-eu");
    expect(p.channels).toHaveLength(40);
    expect(freq("cb-eu", "1")).toBe(26_965_000);
    expect(freq("cb-eu", "9")).toBe(27_065_000);
    expect(freq("cb-eu", "19")).toBe(27_185_000);
    expect(freq("cb-eu", "23")).toBe(27_255_000);
    expect(freq("cb-eu", "24")).toBe(27_235_000);
    expect(freq("cb-eu", "25")).toBe(27_245_000);
    expect(freq("cb-eu", "40")).toBe(27_405_000);
    expect(p.channels.find((c) => c.number === "9")?.name).toMatch(/emergency/i);
  });

  it("the German extension is 41-80 on 26.565-26.955 MHz, FM", () => {
    const p = plan("cb-de-ext");
    expect(p.channels).toHaveLength(40);
    expect(freq("cb-de-ext", "41")).toBe(26_565_000);
    expect(freq("cb-de-ext", "80")).toBe(26_955_000);
    expect(p.mode).toBe("nfm");
    expect(p.regions).toEqual(["DE"]);
  });
});

describe("DAB Band III", () => {
  it("has the 38 blocks 5A-13F with ETSI centre frequencies", () => {
    const p = plan("dab-band3");
    expect(p.channels).toHaveLength(38);
    expect(p.mode).toBe("dab");
    expect(freq("dab-band3", "5A")).toBe(174_928_000);
    expect(freq("dab-band3", "5C")).toBe(178_352_000);
    expect(freq("dab-band3", "10B")).toBe(211_648_000);
    expect(freq("dab-band3", "12D")).toBe(229_072_000);
    expect(freq("dab-band3", "13F")).toBe(239_200_000);
    expect(p.bandwidthHz).toBe(1_536_000);
    expect(p.note).toMatch(/scan/i);
  });

  it("blocks ascend and never overlap (1.536 MHz wide)", () => {
    const f = plan("dab-band3").channels.map((c) => c.freqHz);
    for (let i = 1; i < f.length; i++) {
      expect(
        (f[i] ?? 0) - (f[i - 1] ?? 0),
        plan("dab-band3").channels[i]?.number,
      ).toBeGreaterThanOrEqual(1_536_000);
    }
  });
});

describe("air band", () => {
  it("is a step mode: two rasters and no channel list beyond the guard channel", () => {
    const p = plan("airband");
    expect(p.channels.map((c) => c.number)).toEqual(["Guard"]);
    const [r25, r833] = p.rasters;
    expect(r25 && rasterCount(r25)).toBe(760);
    expect(r833 && rasterCount(r833)).toBe(2280);
  });

  it("names 8.33 kHz channels like pilots do", () => {
    const r833 = plan("airband").rasters[1];
    if (!r833) throw new Error("no 8.33 raster");
    const names = Array.from({ length: 8 }, (_, i) => rasterName(r833, i));
    expect(names).toEqual([
      "118.005",
      "118.010",
      "118.015",
      "118.030",
      "118.035",
      "118.040",
      "118.055",
      "118.060",
    ]);
    expect([0, 1, 2, 3].map((i) => rasterFreq(r833, i))).toEqual([
      118_000_000, 118_008_333, 118_016_667, 118_025_000,
    ]);
    expect(rasterName(r833, 2279)).toBe("136.990");
    expect(rasterFreq(r833, 2279)).toBe(136_991_667);
  });
});

describe("the spectrum overlay copy", () => {
  // The overlay (bandplan/data.ts) keeps its own typed copy of the fixed channel grids. They must
  // agree with the plans of the Channels tab.
  const overlay = (id: string) => {
    const def = CHANNEL_PLANS.find((p) => p.id === id);
    if (!def) throw new Error(`no overlay plan ${id}`);
    return expandPlan(def).map((c) => c.freqHz);
  };
  const sorted = (hz: number[]) => [...hz].sort((a, b) => a - b);
  const of = (id: string) => plan(id).channels.map((c) => c.freqHz);

  it.each([
    ["pmr446", "pmr446"],
    ["freenet", "freenet-de"],
    ["lpd433", "lpd433"],
    ["cb-standard", "cb-eu"],
    ["cb-extended", "cb-de-ext"],
    ["dab", "dab-band3"],
  ])("overlay %s equals plan %s", (overlayId, planId) => {
    expect(sorted(overlay(overlayId))).toEqual(sorted(of(planId)));
  });
});
