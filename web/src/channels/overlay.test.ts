import { describe, expect, it } from "vitest";
import { buildBandPlan, builtinBandPlan, matchBandPlan } from "../bandplan/model";
import { overlayPlans } from "./overlay";
import { parsePlan } from "./schema";

describe("channel plans on the spectrum overlay", () => {
  it("draws the full marine table now", () => {
    const marine = builtinBandPlan().channels.filter((c) => c.planId === "marine");
    expect(marine).toHaveLength(57);
    const m = matchBandPlan(builtinBandPlan(), 156_800_000);
    expect(m.channel?.name).toBe("Marine ch 16");
    expect(m.channel?.note).toMatch(/distress/i);
    expect(matchBandPlan(builtinBandPlan(), 156_450_000).channel?.name).toBe("Marine ch 09");
  });

  it("adds custom plans as imported channels, without the DAB blocks' mode", () => {
    const plan = parsePlan(
      {
        v: 1,
        name: "Club",
        mode: "nfm",
        bandwidth_hz: 12_500,
        channels: [
          { number: "1", name: "Call", freq_hz: 160_100_000 },
          { number: "2", freq_hz: 160_125_000, mode: "am" },
          { number: "3", freq_hz: 178_352_000, mode: "dab" },
        ],
        rasters: [{ start_hz: 1, end_hz: 2, step_hz: 1 }],
      },
      { id: "custom:a" },
    );
    const [p] = overlayPlans([plan]);
    expect(p).toMatchObject({ kind: "list", id: "x:custom:a", category: "custom", mode: "nfm" });
    const merged = buildBandPlan({ ranges: [], plans: overlayPlans([plan]) });
    const hit = matchBandPlan(merged, 160_100_000);
    expect(hit.channel).toMatchObject({ name: "Call", category: "custom", mode: "nfm" });
    expect(matchBandPlan(merged, 160_125_000).channel?.mode).toBe("am");
    expect(matchBandPlan(merged, 178_352_000).channel?.mode).toBeUndefined();
  });

  it("skips raster-only plans", () => {
    const plan = parsePlan(
      { v: 1, name: "Grid", rasters: [{ start_hz: 1, end_hz: 100, step_hz: 1 }] },
      { id: "custom:g" },
    );
    expect(overlayPlans([plan])).toEqual([]);
  });
});
