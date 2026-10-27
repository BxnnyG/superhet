import { describe, expect, it } from "vitest";
import { builtinPlans } from "./catalog";
import {
  MAX_TOLERANCE_HZ,
  MIN_TOLERANCE_HZ,
  buildIndex,
  channelTarget,
  chipLabel,
  matchChannel,
  stepChannel,
  stepChannelBy,
  toleranceFor,
  type MatchOptions,
} from "./lookup";
import { parsePlan, type Plan } from "./schema";

const index = buildIndex(builtinPlans());
const opts: MatchOptions = { region: "DE", rasters: {} };
const at = (hz: number, o: MatchOptions = opts) => matchChannel(index, hz, o);
const label = (hz: number, o?: MatchOptions) => {
  const m = at(hz, o);
  return m ? chipLabel(m.plan, m.channel) : null;
};
const plan = (id: string): Plan => {
  const p = builtinPlans().find((x) => x.id === id);
  if (!p) throw new Error(id);
  return p;
};

describe("tolerance", () => {
  it("is a quarter of the spacing between 0.5 and 3 kHz", () => {
    expect(toleranceFor(12_500)).toBe(3_000);
    expect(toleranceFor(10_000)).toBe(2_500);
    expect(toleranceFor(8_333)).toBeCloseTo(2_083.25);
    expect(toleranceFor(1_000)).toBe(MIN_TOLERANCE_HZ);
    expect(toleranceFor(1e6)).toBe(MAX_TOLERANCE_HZ);
    expect(toleranceFor(Infinity)).toBe(MAX_TOLERANCE_HZ);
  });
});

describe("lookup by frequency", () => {
  it("finds the channel under the frequency", () => {
    expect(label(446_031_250)).toBe("PMR 3");
    expect(label(156_800_000)).toBe("Marine 16");
    expect(label(149_100_000)).toBe("Freenet 5");
    expect(label(433_075_000)).toBe("LPD 1");
    expect(label(27_185_000)).toBe("CB 19");
  });

  it("accepts a slightly detuned VFO but not one between channels", () => {
    expect(label(446_031_250 + 2_900)).toBe("PMR 3");
    expect(label(446_031_250 - 2_900)).toBe("PMR 3");
    expect(label(446_031_250 + 3_100)).toBeNull(); // 3.1 kHz off, mid-gap towards ch 4 is 6.25
    expect(label(446_037_500)).toBeNull();
    expect(label(100_000_000)).toBeNull();
  });

  it("reports the signed offset", () => {
    expect(at(446_031_250 + 1_200)?.offsetHz).toBe(1_200);
    expect(at(446_031_250 - 700)?.offsetHz).toBe(-700);
  });

  it("does not match DAB blocks (no VFO sits there) but does the CB channels out of order", () => {
    expect(label(178_352_000)).toBeNull();
    expect(label(27_235_000)).toBe("CB 24");
    expect(label(27_255_000)).toBe("CB 23");
  });

  it("prefers the nearest channel, then the plan of the user's region", () => {
    const twin = parsePlan({
      v: 1,
      id: "twin",
      name: "Twin",
      chip: "Twin",
      regions: ["US"],
      channels: [{ number: "1", freq_hz: 446_031_250 }],
    });
    const idx = buildIndex([twin, ...builtinPlans()]);
    // Exactly the same frequency: DE users get PMR, US users the twin.
    const de = matchChannel(idx, 446_031_250, { region: "DE", rasters: {} });
    expect(de?.plan.id).toBe("pmr446");
    const us = matchChannel(idx, 446_031_250, { region: "US", rasters: {} });
    expect(us?.plan.id).toBe("twin");
    // A nearer channel beats the region.
    const near = parsePlan({
      v: 1,
      id: "near",
      name: "Near",
      regions: ["US"],
      channels: [{ number: "9", freq_hz: 446_031_250 + 100 }],
    });
    const idx2 = buildIndex([...builtinPlans(), near]);
    expect(matchChannel(idx2, 446_031_250 + 90, { region: "DE", rasters: {} })?.plan.id).toBe(
      "near",
    );
  });
});

describe("air band rasters", () => {
  it("matches the active raster only", () => {
    expect(label(118_025_000)).toBe("Air 118.025");
    expect(label(118_008_333, opts)).toBeNull();
    const fine = { region: "DE", rasters: { airband: "8.33k" } };
    expect(label(118_008_333, fine)).toBe("Air 118.010");
    expect(label(118_000_000, fine)).toBe("Air 118.005");
    expect(label(118_025_000, fine)).toBe("Air 118.030");
  });

  it("stays out of the band edges and keeps the guard channel", () => {
    expect(label(117_900_000)).toBeNull();
    expect(label(137_100_000)).toBeNull();
    expect(label(121_500_000)).toBe("Air Guard");
  });

  it("steps along the grid", () => {
    const m = at(118_025_000);
    expect(m && stepChannel(m, 1)?.channel.freqHz).toBe(118_050_000);
    expect(m && stepChannel(m, -1)?.channel.freqHz).toBe(118_000_000);
    const fine = at(118_000_000, { region: "DE", rasters: { airband: "8.33k" } });
    expect(fine && stepChannelBy(fine, 3)?.channel.freqHz).toBe(118_025_000);
    expect(fine && stepChannelBy(fine, 1)?.channel.number).toBe("118.010");
    const first = at(118_000_000);
    expect(first && stepChannel(first, -1)).toBeNull();
    const last = at(136_975_000);
    expect(last && stepChannel(last, 1)).toBeNull();
  });
});

describe("channel stepping", () => {
  it("steps in plan order and stops at the ends", () => {
    const pmr3 = at(446_031_250);
    expect(pmr3 && stepChannel(pmr3, 1)?.channel.number).toBe("4");
    expect(pmr3 && stepChannel(pmr3, -1)?.channel.freqHz).toBe(446_018_750);
    const first = at(446_006_250);
    expect(first && stepChannel(first, -1)).toBeNull();
    const last = at(446_193_750);
    expect(last && stepChannel(last, 1)).toBeNull();
  });

  it("follows the channel numbers where the CB frequencies are not monotonic", () => {
    const cb22 = at(27_225_000);
    const cb23 = cb22 && stepChannel(cb22, 1);
    expect(cb23?.channel.number).toBe("23");
    expect(cb23?.channel.freqHz).toBe(27_255_000);
    const cb24 = cb23 && stepChannel(cb23, 1);
    expect(cb24?.channel.freqHz).toBe(27_235_000);
  });

  it("steps several channels at once and clamps at the end", () => {
    const pmr3 = at(446_031_250);
    expect(pmr3 && stepChannelBy(pmr3, 10)?.channel.number).toBe("13");
    expect(pmr3 && stepChannelBy(pmr3, 100)?.channel.number).toBe("16");
    expect(pmr3 && stepChannelBy(pmr3, -10)?.channel.number).toBe("1");
    const last = at(446_193_750);
    expect(last && stepChannelBy(last, 5)).toBeNull();
  });

  it("walks the marine table by channel number", () => {
    const ch16 = at(156_800_000);
    expect(ch16 && stepChannel(ch16, 1)?.channel.number).toBe("17");
    expect(ch16 && stepChannel(ch16, -1)?.channel.number).toBe("15");
  });
});

describe("channel target", () => {
  it("carries mode, bandwidth and step of the plan", () => {
    const p = plan("pmr446");
    expect(channelTarget(p, p.channels[2]!)).toEqual({
      freqHz: 446_031_250,
      mode: "nfm",
      bandwidthHz: 12_500,
      stepHz: 12_500,
    });
  });

  it("a channel can override mode and add a CTCSS tone", () => {
    const p = parsePlan({
      v: 1,
      id: "x",
      name: "X",
      mode: "nfm",
      step_hz: 7,
      channels: [
        { number: "1", freq_hz: 145_500_000, mode: "am", bandwidth_hz: 9_000, ctcss_hz: 88.5 },
        { number: "2", freq_hz: 145_525_000, mode: "usb", ctcss_hz: 88.5 },
      ],
    });
    expect(channelTarget(p, p.channels[0]!)).toEqual({
      freqHz: 145_500_000,
      mode: "am",
      bandwidthHz: 9_000,
      toneSquelchHz: 88.5,
    });
    // No tone squelch on SSB; a step that is not offered is not requested.
    expect(channelTarget(p, p.channels[1]!)).toEqual({ freqHz: 145_525_000, mode: "usb" });
  });

  it("has no VFO target for DAB blocks", () => {
    const p = plan("dab-band3");
    expect(channelTarget(p, p.channels[0]!)).toBeNull();
  });
});
