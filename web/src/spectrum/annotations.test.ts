import { describe, expect, it } from "vitest";
import { CATEGORIES } from "../bandplan/data";
import { builtinBandPlan } from "../bandplan/model";
import { newInput, type Bookmark } from "../bookmarks/model";
import {
  AnnotationLayer,
  CHANNEL_LANE_H,
  EMPTY_LAYOUT,
  MAX_LANES,
  assignLanes,
  hitStrip,
  hitTarget,
  hitTooltip,
  layoutStrip,
  wrapText,
} from "./annotations";

const plan = builtinBandPlan();
const all = new Set(CATEGORIES.map((c) => c.id));
const measure = (t: string) => t.length * 5;

const bookmark = (over: Partial<Bookmark> = {}): Bookmark => ({
  ...newInput({
    name: "Home repeater",
    freqHz: 438_650_000,
    mode: "nfm",
    bandwidthHz: 12_500,
    tags: ["ham"],
  }),
  id: "1",
  createdMs: 0,
  updatedMs: 0,
  ...over,
});

const strip = (startHz: number, endHz: number, bookmarks: Bookmark[] = [], plotW = 1000) =>
  layoutStrip({ plan, enabled: all, bookmarks, span: { startHz, endHz }, plotW, measure });

describe("layoutStrip", () => {
  it("is empty where the plan has nothing", () => {
    const l = strip(500e6, 502.4e6);
    expect(l.insetPx).toBe(0);
    expect(l.bars).toHaveLength(0);
    expect(strip(500e6, 502.4e6, [], 0)).toBe(EMPTY_LAYOUT);
  });

  it("stacks overlapping ranges in lanes and never exceeds the lane cap", () => {
    const l = strip(432.5e6, 435e6);
    expect(l.lanes).toBeGreaterThanOrEqual(2);
    expect(l.lanes).toBeLessThanOrEqual(MAX_LANES);
    for (const a of l.bars) {
      for (const b of l.bars) {
        if (a === b || a.lane !== b.lane) continue;
        expect(a.x1 <= b.x0 || b.x1 <= a.x0, `${a.range.id} / ${b.range.id}`).toBe(true);
      }
    }
  });

  it("drops what does not fit the lanes", () => {
    const base = plan.ranges[0];
    if (!base) throw new Error("fixture");
    const ranges = [0, 1, 2, 3].map((i) => ({ ...base, id: `r${i}`, startHz: 1, endHz: 100 }));
    expect(assignLanes(ranges)).toHaveLength(MAX_LANES);
  });

  it("shows a label only when it fits inside the bar", () => {
    const wide = strip(87e6, 89e6).bars.find((b) => b.range.id === "fm-broadcast");
    expect(wide?.label).toBe("FM broadcast");
    const narrow = strip(446e6, 446.2e6, [], 1000).bars.find((b) => b.range.id === "pmr446");
    expect(narrow?.label).toBe("PMR446");
    const tiny = strip(440e6, 460e6, [], 1000).bars.find((b) => b.range.id === "pmr446");
    expect(tiny?.label).toBeNull();
  });

  it("clips bars to the visible span", () => {
    const bar = strip(100e6, 110e6).bars.find((b) => b.range.id === "fm-broadcast");
    expect(bar?.x0).toBe(0);
    expect(bar?.x1).toBeCloseTo(800, 5);
  });

  it("adds a channel lane for ticks and bookmarks", () => {
    const plain = strip(445e6, 447e6);
    expect(plain.hasChannelLane).toBe(true);
    expect(plain.ticks.filter((t) => t.channel.planId === "pmr446")).toHaveLength(16);
    const none = strip(500e6, 502e6, [bookmark({ freqHz: 501e6 })]);
    expect(none.hasChannelLane).toBe(true);
    expect(none.insetPx).toBeGreaterThanOrEqual(CHANNEL_LANE_H);
    expect(none.marks).toHaveLength(1);
  });

  it("labels channels only when they have room and bookmarks win label space", () => {
    const l = strip(446.0e6, 446.2e6);
    expect(l.ticks.filter((t) => t.label !== null)).toHaveLength(16);
    const crowded = strip(446.0e6, 446.2e6, [
      bookmark({ freqHz: 446_050_000, name: "Long bookmark name" }),
    ]);
    expect(crowded.marks[0]?.label).toBe("Long bookmark name");
    expect(crowded.ticks.filter((t) => t.label !== null).length).toBeLessThan(16);
  });

  it("omits bookmarks outside the span", () => {
    expect(strip(500e6, 502e6, [bookmark({ freqHz: 600e6 })]).marks).toHaveLength(0);
  });
});

describe("hit testing", () => {
  const bm = bookmark({ freqHz: 446_050_000 });
  const l = strip(445.9e6, 446.3e6, [bm], 1000);
  const pmr5 = l.ticks.find((t) => t.channel.planId === "pmr446" && t.channel.label === "5");

  it("finds a channel tick in the channel lane", () => {
    expect(pmr5).toBeDefined();
    const hit = hitStrip(l, (pmr5?.x ?? 0) + 1, l.channelTop + 3);
    expect(hit?.kind).toBe("channel");
    expect(hit?.kind === "channel" && hit.channel.label).toBe("5");
  });

  it("prefers a bookmark over a channel at the same place", () => {
    const hit = hitStrip(l, l.marks[0]?.x ?? 0, l.channelTop + 3);
    expect(hit?.kind).toBe("bookmark");
  });

  it("finds a range bar by lane and nothing below the strip", () => {
    const bar = l.bars.find((b) => b.range.id === "pmr446");
    expect(bar).toBeDefined();
    const x = ((bar?.x0 ?? 0) + (bar?.x1 ?? 0)) / 2;
    const y = 2 + (bar?.lane ?? 0) * 11 + 4;
    expect(hitStrip(l, x, y)?.kind).toBe("range");
    expect(hitStrip(l, 10, l.insetPx + 1)).toBeNull();
    expect(hitStrip(l, 10, -1)).toBeNull();
  });

  it("only tunable hits produce a target", () => {
    const channel = plan.channels.find((c) => c.key === "pmr446:446006250");
    const adsb = plan.channels.find((c) => c.planId === "adsb");
    const range = plan.ranges[0];
    if (!channel || !adsb || !range) throw new Error("fixture");
    expect(hitTarget({ kind: "channel", channel, x: 0 })).toEqual({
      freqHz: 446_006_250,
      mode: "nfm",
      bandwidthHz: 12_500,
    });
    expect(hitTarget({ kind: "channel", channel: adsb, x: 0 })).toBeNull();
    expect(hitTarget({ kind: "range", range, x: 0 })).toBeNull();
    const plain = bookmark({ mode: null, bandwidthHz: null });
    expect(hitTarget({ kind: "bookmark", bookmark: plain, x: 0 })).toEqual({
      freqHz: 438_650_000,
      toneSquelchHz: null,
    });
    // A bookmark applies its whole receiver state.
    const full = bookmark({
      squelchDb: -80,
      stepHz: 25_000,
      toneMode: "ctcss",
      ctcssHz: 88.5,
    });
    expect(hitTarget({ kind: "bookmark", bookmark: full, x: 0 })).toEqual({
      freqHz: 438_650_000,
      mode: "nfm",
      bandwidthHz: 12_500,
      squelchDbfs: -80,
      stepHz: 25_000,
      toneSquelchHz: 88.5,
    });
    expect(hitTarget(null)).toBeNull();
  });
});

describe("tooltips", () => {
  it("describes a channel with frequency, mode, bandwidth and note", () => {
    const channel = plan.channels.find((c) => c.key === "cb-standard:27065000");
    if (!channel) throw new Error("fixture");
    const tip = hitTooltip({ kind: "channel", channel, x: 0 });
    expect(tip.title).toBe("CB ch 9");
    expect(tip.lines[0]).toContain("27.065 000 MHz");
    expect(tip.lines[0]).toContain("AM / FM");
    expect(tip.lines.join(" ")).toContain("Emergency");
  });

  it("flags approximate ranges and the BOS notice", () => {
    const biz = plan.ranges.find((r) => r.id === "biz-2m");
    const bos = plan.ranges.find((r) => r.category === "tetra");
    if (!biz || !bos) throw new Error("fixture");
    expect(hitTooltip({ kind: "range", range: biz, x: 0 }).lines.join(" ")).toContain(
      "Approximate",
    );
    expect(hitTooltip({ kind: "range", range: bos, x: 0 }).title).toContain(
      "reception not permitted",
    );
  });

  it("describes a bookmark", () => {
    const tip = hitTooltip({
      kind: "bookmark",
      bookmark: bookmark({ notes: "Local", toneMode: "ctcss", ctcssHz: 88.5 }),
      x: 0,
    });
    expect(tip.title).toBe("Home repeater");
    expect(tip.lines.join("|")).toContain("NFM");
    expect(tip.lines.join("|")).toContain("88.5");
    expect(tip.lines).toContain("Local");
  });

  it("wraps long text by words", () => {
    expect(wrapText("aaa bbb ccc", 30, measure)).toEqual(["aaa", "bbb", "ccc"]);
    expect(wrapText("aaa bbb ccc", 1000, measure)).toEqual(["aaa bbb ccc"]);
  });
});

describe("AnnotationLayer", () => {
  it("honours disabled categories and hidden bookmarks", () => {
    const layer = new AnnotationLayer();
    const span = { startHz: 445.9e6, endHz: 446.3e6 };
    expect(layer.inset).toBe(0);
    layer.setEnabled(new Set(["pmr446" as const]));
    layer.setBookmarks([bookmark({ freqHz: 446_050_000 })], false);
    layer.update(null, span, 1000, 1);
    expect(layer.inset).toBeGreaterThan(0);
    expect(layer.hit(500, 3)?.kind).toBe("range");
    layer.setEnabled(new Set());
    layer.update(null, span, 1000, 1);
    expect(layer.inset).toBe(0);
    layer.setBookmarks([bookmark({ freqHz: 446_050_000 })], true);
    layer.update(null, span, 1000, 1);
    expect(layer.inset).toBeGreaterThan(0);
  });
});
