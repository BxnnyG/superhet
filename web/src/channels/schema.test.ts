import { describe, expect, it } from "vitest";
import { builtinPlans } from "./catalog";
import {
  HF_LIMIT_HZ,
  PlanError,
  needsHf,
  normalizeNumber,
  parsePlan,
  planDocument,
} from "./schema";

const channel = (over: Record<string, unknown> = {}) => ({
  number: "1",
  freq_hz: 145_500_000,
  ...over,
});
const doc = (over: Record<string, unknown> = {}) => ({
  v: 1,
  id: "club",
  name: "Club",
  channels: [channel()],
  ...over,
});

function reason(raw: unknown): string {
  try {
    parsePlan(raw);
  } catch (e) {
    expect(e).toBeInstanceOf(PlanError);
    return (e as Error).message;
  }
  throw new Error("expected the plan to be rejected");
}

describe("plan schema", () => {
  it("fills in the defaults of a minimal plan", () => {
    const p = parsePlan(doc());
    expect(p).toMatchObject({
      id: "club",
      service: "Club",
      chip: "Club",
      regions: ["WORLD"],
      licence: "receive-only",
      mode: "nfm",
      builtin: false,
    });
    expect(p.channels[0]).toMatchObject({ number: "1", freqHz: 145_500_000, ctcssHz: null });
  });

  it("numbers channels without a number by position and accepts numeric numbers", () => {
    const p = parsePlan(
      doc({ channels: [{ freq_hz: 100_000_000 }, { number: 7, freq_hz: 100_100_000 }] }),
    );
    expect(p.channels.map((c) => c.number)).toEqual(["1", "7"]);
  });

  it("a custom plan takes the given id, not the one in the document", () => {
    expect(parsePlan(doc(), { id: "custom:abc" }).id).toBe("custom:abc");
  });

  it.each([
    [{ v: 2 }, "v"],
    [{ id: "" }, "id"],
    [{ id: "bad id" }, "id"],
    [{ name: " " }, "name"],
    [{ regions: ["de"] }, "regions"],
    [{ regions: "DE" }, "regions"],
    [{ licence: "free" }, "licence"],
    [{ mode: "dmr" }, "mode"],
    [{ bandwidth_hz: -1 }, "bandwidth_hz"],
    [{ deemphasis: "25us" }, "deemphasis"],
    [{ aliases: ["Has Space"] }, "aliases"],
    [{ channels: [] }, "channels"],
    [{ channels: "x" }, "channels"],
    [{ channels: [channel({ freq_hz: 145.5 })] }, "freq_hz"],
    [{ channels: [channel({ freq_hz: 0 })] }, "freq_hz"],
    [{ channels: [channel({ freq_hz: "145500000" })] }, "freq_hz"],
    [{ channels: [channel({ ctcss_hz: 90 })] }, "ctcss_hz"],
    [{ channels: [channel({ mode: "x" })] }, "mode"],
    [{ channels: [channel(), channel({ number: "01" })] }, "duplicate"],
    [{ channels: undefined, rasters: [{ start_hz: 2, end_hz: 1, step_hz: 1 }] }, "end_hz"],
    [{ channels: undefined, rasters: [{ start_hz: 1, end_hz: 1e9, step_hz: 1 }] }, "at most"],
    [{ channels: undefined, rasters: [{ start_hz: 1, end_hz: 5, step_hz: 0 }] }, "step_hz"],
    [
      { channels: undefined, rasters: [{ start_hz: 1, end_hz: 5, step_hz: 1, naming: "x" }] },
      "naming",
    ],
  ])("rejects %j (%s)", (over, word) => {
    expect(reason(doc(over))).toContain(word);
  });

  it("rejects non-objects", () => {
    expect(reason(null)).toContain("object");
    expect(reason([])).toContain("object");
  });

  it("accepts a raster-only plan and a mode per channel", () => {
    const p = parsePlan(
      doc({
        channels: [channel({ mode: "dab", bandwidth_hz: 1_536_000 })],
        rasters: [{ id: "g", start_hz: 118_000_000, end_hz: 136_975_000, step_hz: 25_000 }],
      }),
    );
    expect(p.channels[0]?.mode).toBe("dab");
    expect(p.rasters[0]).toMatchObject({ label: "25 kHz", naming: "plain" });
  });

  it("normalises channel numbers for comparison", () => {
    expect(normalizeNumber("05")).toBe("5");
    expect(normalizeNumber(" 5C ")).toBe("5c");
    expect(normalizeNumber("87B")).toBe("87b");
    expect(normalizeNumber("0")).toBe("0");
  });

  it("round-trips through the wire form", () => {
    const original = parsePlan(
      doc({
        regions: ["DE", "EU"],
        licence: "licensed",
        licence_note: "n",
        step_hz: 12_500,
        aliases: ["club"],
        channels: [
          channel({ name: "Call", ctcss_hz: 88.5, note: "x", mode: "am", bandwidth_hz: 9000 }),
        ],
        rasters: [
          { id: "a", label: "A", start_hz: 1, end_hz: 100, step_hz: 10, naming: "icao-8.33" },
        ],
      }),
    );
    expect(parsePlan(planDocument(original), { id: original.id })).toEqual(original);
  });

  it("flags plans in the HF range", () => {
    expect(needsHf(parsePlan(doc({ channels: [channel({ freq_hz: HF_LIMIT_HZ - 1 })] })))).toBe(
      true,
    );
    expect(needsHf(parsePlan(doc()))).toBe(false);
  });
});

describe("built-in plans", () => {
  it("all validate and have unique ids", () => {
    const plans = builtinPlans();
    expect(plans.length).toBeGreaterThanOrEqual(8);
    expect(new Set(plans.map((p) => p.id)).size).toBe(plans.length);
    for (const p of plans) {
      expect(p.builtin).toBe(true);
      expect(p.source, p.id).not.toBeNull();
      expect(p.licenceNote, p.id).not.toBeNull();
    }
  });
});
