import { describe, expect, it } from "vitest";
import { CATEGORIES, CHANNEL_PLANS, RANGES } from "./data";
import {
  MIN_LABEL_PX,
  MIN_TICK_PX,
  builtinBandPlan,
  channelDetail,
  expandPlan,
  matchBandPlan,
  parseExtraBandPlan,
  suggestName,
  visibleChannels,
  visibleRanges,
} from "./model";
import { OverlayStore, enabledCategories, loadOverlay, DEFAULT_OVERLAY } from "./settings";

const plan = builtinBandPlan();
const all = new Set(CATEGORIES.map((c) => c.id));
const channelsOf = (id: string) => plan.channels.filter((c) => c.planId === id);
const planDef = (id: string) => {
  const p = CHANNEL_PLANS.find((x) => x.id === id);
  if (!p) throw new Error(`no plan ${id}`);
  return expandPlan(p);
};

describe("band plan data", () => {
  it("has unique ranges that start before they end, in known categories", () => {
    const ids = new Set<string>();
    for (const r of RANGES) {
      expect(ids.has(r.id), r.id).toBe(false);
      ids.add(r.id);
      expect(r.endHz, r.id).toBeGreaterThan(r.startHz);
      expect(all.has(r.category), r.id).toBe(true);
      expect(r.source, r.id).not.toBe("");
    }
  });

  it("is sorted by start frequency within every category", () => {
    for (const cat of CATEGORIES) {
      const starts = RANGES.filter((r) => r.category === cat.id).map((r) => r.startHz);
      expect(starts, cat.id).toEqual([...starts].sort((a, b) => a - b));
    }
    const merged = plan.ranges.map((r) => r.startHz);
    expect(merged).toEqual([...merged].sort((a, b) => a - b));
  });

  it("only overlaps within a category for declared sub-segments", () => {
    const byId = new Map(RANGES.map((r) => [r.id, r]));
    for (const r of RANGES) {
      if (r.parent === undefined) continue;
      const parent = byId.get(r.parent);
      expect(parent, `${r.id} parent`).toBeDefined();
      expect(r.startHz, r.id).toBeGreaterThanOrEqual(parent?.startHz ?? Infinity);
      expect(r.endHz, r.id).toBeLessThanOrEqual(parent?.endHz ?? 0);
      expect(parent?.category).toBe(r.category);
    }
    for (const a of RANGES) {
      for (const b of RANGES) {
        if (a === b || a.category !== b.category || a.id >= b.id) continue;
        const overlap = a.startHz < b.endHz && b.startHz < a.endHz;
        const nested = a.parent === b.id || b.parent === a.id;
        const siblings = a.parent !== undefined && a.parent === b.parent;
        if (overlap) expect(nested, `${a.id} vs ${b.id}`).toBe(true);
        if (siblings) expect(overlap, `${a.id} vs ${b.id}`).toBe(false);
      }
    }
  });

  it("keeps channels sorted and unique", () => {
    const freqs = plan.channels.map((c) => c.freqHz);
    expect(freqs).toEqual([...freqs].sort((a, b) => a - b));
    expect(new Set(plan.channels.map((c) => c.key)).size).toBe(plan.channels.length);
  });

  it("marks the BOS TETRA ranges with the reception notice", () => {
    const bos = RANGES.filter((r) => r.category === "tetra");
    expect(bos.map((r) => [r.startHz, r.endHz])).toEqual([
      [380e6, 385e6],
      [390e6, 395e6],
    ]);
    for (const r of bos) expect(r.name).toContain("reception not permitted");
  });
});

describe("channel math", () => {
  it("PMR446: 16 channels from 446.00625 MHz in 12.5 kHz steps", () => {
    const ch = planDef("pmr446");
    expect(ch).toHaveLength(16);
    expect(ch[0]?.freqHz).toBe(446_006_250);
    expect(ch[7]?.freqHz).toBe(446_093_750);
    expect(ch[15]?.freqHz).toBe(446_193_750);
    expect(ch[0]).toMatchObject({ label: "1", mode: "nfm", bandwidthHz: 12_500 });
  });

  it("LPD433: 69 channels from 433.075 MHz in 25 kHz steps", () => {
    const ch = planDef("lpd433");
    expect(ch).toHaveLength(69);
    expect(ch[0]?.freqHz).toBe(433_075_000);
    expect(ch[68]?.freqHz).toBe(434_775_000);
    expect(ch[68]?.label).toBe("69");
  });

  it("Freenet: the six channels", () => {
    expect(planDef("freenet").map((c) => c.freqHz)).toEqual([
      149_025_000, 149_037_500, 149_050_000, 149_087_500, 149_100_000, 149_112_500,
    ]);
  });

  it("CB: 40 standard channels plus the 41-80 extension", () => {
    const std = planDef("cb-standard");
    expect(std).toHaveLength(40);
    const byLabel = (l: string) => std.find((c) => c.label === l)?.freqHz;
    expect(byLabel("1")).toBe(26_965_000);
    expect(byLabel("9")).toBe(27_065_000);
    expect(byLabel("19")).toBe(27_185_000);
    expect(byLabel("23")).toBe(27_255_000);
    expect(byLabel("24")).toBe(27_235_000);
    expect(byLabel("40")).toBe(27_405_000);
    const ext = planDef("cb-extended");
    expect(ext).toHaveLength(40);
    expect(ext[0]).toMatchObject({ label: "41", freqHz: 26_565_000 });
    expect(ext[39]).toMatchObject({ label: "80", freqHz: 26_955_000 });
    expect(channelsOf("cb-standard").concat(channelsOf("cb-extended"))).toHaveLength(80);
  });

  it("DAB: block centre frequencies", () => {
    const dab = planDef("dab");
    expect(dab).toHaveLength(38);
    expect(dab.find((c) => c.label === "5A")?.freqHz).toBe(174_928_000);
    expect(dab.find((c) => c.label === "12D")?.freqHz).toBe(229_072_000);
    expect(dab.find((c) => c.label === "13F")?.freqHz).toBe(239_200_000);
  });

  it("only channels with a VFO mode are tunable", () => {
    expect(plan.channels.find((c) => c.key === "adsb:1090000000")?.mode).toBeUndefined();
    expect(plan.channels.find((c) => c.key === "air-guard:121500000")?.mode).toBe("am");
  });
});

describe("visibility thresholds", () => {
  it("hides, ticks or labels channels by on-screen spacing", () => {
    const hzPerPx = 1000;
    expect(channelDetail(MIN_TICK_PX * hzPerPx - 1, hzPerPx)).toBe("hidden");
    expect(channelDetail(MIN_TICK_PX * hzPerPx, hzPerPx)).toBe("tick");
    expect(channelDetail(MIN_LABEL_PX * hzPerPx - 1, hzPerPx)).toBe("tick");
    expect(channelDetail(MIN_LABEL_PX * hzPerPx, hzPerPx)).toBe("label");
    expect(channelDetail(Infinity, hzPerPx)).toBe("label");
    expect(channelDetail(12_500, 0)).toBe("hidden");
  });

  it("shows PMR446 channels in a 2.4 MHz window but labels them only when zoomed in", () => {
    const wide = { startHz: 445e6, endHz: 447.4e6 };
    const at1000px = visibleChannels(plan, wide, all, 2.4e6 / 1000).filter(
      (v) => v.channel.planId === "pmr446",
    );
    expect(at1000px).toHaveLength(16);
    expect(at1000px.every((v) => v.detail === "tick")).toBe(true);
    const narrow = { startHz: 446.0e6, endHz: 446.2e6 };
    const zoomed = visibleChannels(plan, narrow, all, 0.2e6 / 1000).filter(
      (v) => v.channel.planId === "pmr446",
    );
    expect(zoomed.every((v) => v.detail === "label")).toBe(true);
  });

  it("drops CB channels when 10 kHz is below the tick spacing", () => {
    const span = { startHz: 26.5e6, endHz: 27.5e6 };
    expect(
      visibleChannels(plan, span, all, 1e6 / 300).filter((v) => v.channel.category === "cb"),
    ).toHaveLength(0);
    expect(
      visibleChannels(plan, span, all, 1e6 / 1000).filter((v) => v.channel.category === "cb")
        .length,
    ).toBeGreaterThan(0);
  });

  it("filters by span and enabled categories", () => {
    const span = { startHz: 430e6, endHz: 440e6 };
    const only = new Set(["amateur" as const]);
    const r = visibleRanges(plan, span, only, 10_000);
    expect(r.length).toBeGreaterThan(0);
    expect(r.every((x) => x.category === "amateur")).toBe(true);
    expect(visibleRanges(plan, span, new Set(), 10_000)).toHaveLength(0);
    expect(visibleRanges(plan, { startHz: 1e9, endHz: 1.01e9 }, all, 100)).toHaveLength(0);
  });

  it("skips ranges narrower than a pixel", () => {
    const span = { startHz: 446e6, endHz: 446.2e6 };
    expect(visibleRanges(plan, span, all, 1e9).some((r) => r.id === "pmr446")).toBe(false);
  });
});

describe("matching", () => {
  it("finds the channel and the narrowest range", () => {
    const m = matchBandPlan(plan, 446_093_800);
    expect(m.channel?.name).toBe("PMR446 ch 8");
    expect(m.range?.id).toBe("pmr446");
    expect(suggestName(m)).toBe("PMR446 ch 8");
  });

  it("falls back to the range and then to nothing", () => {
    const m = matchBandPlan(plan, 100_000_000);
    expect(m.channel).toBeNull();
    expect(suggestName(m)).toBe("FM broadcast");
    expect(suggestName(matchBandPlan(plan, 1_700_000_000))).toBe("");
  });

  it("respects the enabled categories", () => {
    const m = matchBandPlan(plan, 446_093_750, new Set(["amateur" as const]));
    expect(m.channel).toBeNull();
  });
});

describe("import hook", () => {
  it("turns a version 1 document into ranges and channels", () => {
    const extra = parseExtraBandPlan(
      {
        v: 1,
        name: "Test",
        ranges: [
          { name: "Foo", start_hz: 100e6, end_hz: 101e6, category: "amateur" },
          { name: "bad", start_hz: 5, end_hz: 1 },
        ],
        channels: [{ name: "Bar", freq_hz: 123_450_000, mode: "am", bandwidth_hz: 8000 }, 3],
      },
      "x",
    );
    expect(extra.ranges).toHaveLength(1);
    expect(extra.ranges[0]).toMatchObject({ category: "amateur", source: "Test" });
    expect(extra.plans).toHaveLength(1);
    const merged = expandPlan(extra.plans[0] ?? CHANNEL_PLANS[0]!);
    expect(merged[0]).toMatchObject({ freqHz: 123_450_000, mode: "am", category: "custom" });
  });

  it("rejects documents of another version", () => {
    expect(() => parseExtraBandPlan({ v: 2 })).toThrow();
    expect(() => parseExtraBandPlan("x")).toThrow();
  });
});

describe("overlay settings", () => {
  it("defaults to everything on except business radio", () => {
    const on = enabledCategories(DEFAULT_OVERLAY);
    expect(on.has("pmr446")).toBe(true);
    expect(on.has("business")).toBe(false);
  });

  it("persists toggles and survives garbage", () => {
    const data = new Map<string, string>();
    const storage = {
      getItem: (k: string) => data.get(k) ?? null,
      setItem: (k: string, v: string) => void data.set(k, v),
    };
    const store = new OverlayStore(DEFAULT_OVERLAY, storage);
    let calls = 0;
    store.subscribe(() => calls++);
    store.setCategory("business", true);
    store.set({ bandPlan: false });
    expect(calls).toBe(2);
    const loaded = loadOverlay(storage);
    expect(loaded.bandPlan).toBe(false);
    expect(loaded.categories.business).toBe(true);
    expect(enabledCategories(loaded).size).toBe(0);
    data.set("superhet.bandplan.v1", "{nope");
    expect(loadOverlay(storage)).toEqual(DEFAULT_OVERLAY);
  });
});
