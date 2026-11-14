import { describe, expect, it } from "vitest";
import { activeFilters, NO_FILTER, passes, toggleGroup, type AircraftFilter } from "./filters";
import { parseStatus, tracked, type Tracked } from "./model";
import { KIND_GROUPS, type SymbolKind } from "./symbols";
import { adsbStatusRaw, aircraftRaw } from "./testing";

function one(over: Record<string, unknown>): Tracked {
  const status = parseStatus(adsbStatusRaw({ aircraft: [aircraftRaw(over)] }));
  const a = status ? tracked(status, 0, 0)[0] : undefined;
  if (!a) throw new Error("fixture");
  return a;
}

const airliner = { kind: "narrow" as SymbolKind, military: false };
const jetFilter = (f: Partial<AircraftFilter>): AircraftFilter => ({ ...NO_FILTER, ...f });

describe("aircraft filter", () => {
  it("lets everything through by default", () => {
    expect(passes(one({}), airliner, NO_FILTER)).toBe(true);
    expect(activeFilters(NO_FILTER)).toBe(0);
  });

  it("cuts by altitude window; the ground counts as 0 ft", () => {
    const high = one({ alt_ft: 35000 });
    const low = one({ alt_ft: 3000 });
    const ground = one({ alt_ft: undefined, on_ground: true });
    const unknown = one({ alt_ft: undefined });
    const above10k = jetFilter({ altMinFt: 10000 });
    expect(passes(high, airliner, above10k)).toBe(true);
    expect(passes(low, airliner, above10k)).toBe(false);
    expect(passes(ground, airliner, above10k)).toBe(false);
    // No altitude reported: cannot be said to be inside a window.
    expect(passes(unknown, airliner, above10k)).toBe(false);
    const below5k = jetFilter({ altMaxFt: 5000 });
    expect(passes(low, airliner, below5k)).toBe(true);
    expect(passes(high, airliner, below5k)).toBe(false);
    expect(passes(ground, airliner, below5k)).toBe(true);
    // Inclusive edges.
    expect(
      passes(one({ alt_ft: 5000 }), airliner, jetFilter({ altMinFt: 5000, altMaxFt: 5000 })),
    ).toBe(true);
    expect(activeFilters(jetFilter({ altMinFt: 1, altMaxFt: 2 }))).toBe(1);
  });

  it("separates ground traffic from airborne", () => {
    const ground = one({ alt_ft: undefined, on_ground: true });
    const air = one({});
    expect(passes(ground, airliner, jetFilter({ ground: "airborne" }))).toBe(false);
    expect(passes(air, airliner, jetFilter({ ground: "airborne" }))).toBe(true);
    expect(passes(ground, airliner, jetFilter({ ground: "ground" }))).toBe(true);
    expect(passes(air, airliner, jetFilter({ ground: "ground" }))).toBe(false);
  });

  it("keeps only military aircraft when asked (flag from the database, or a fast jet)", () => {
    const f = jetFilter({ militaryOnly: true });
    expect(passes(one({}), airliner, f)).toBe(false);
    expect(passes(one({}), { kind: "narrow", military: true }, f)).toBe(true);
    expect(passes(one({}), { kind: "fighter", military: false }, f)).toBe(true);
  });

  it("hides whole type groups", () => {
    const noHelis = jetFilter({ hiddenGroups: ["rotor"] });
    expect(passes(one({}), { kind: "heli", military: false }, noHelis)).toBe(false);
    expect(passes(one({}), airliner, noHelis)).toBe(true);
    const noAirliners = jetFilter({ hiddenGroups: ["airliner"] });
    for (const kind of ["narrow", "wide", "quad"] as const) {
      expect(passes(one({}), { kind, military: false }, noAirliners)).toBe(false);
    }
    expect(passes(one({}), { kind: "light", military: false }, noAirliners)).toBe(true);
  });

  it("toggles groups and never hides all of them", () => {
    let f = toggleGroup(NO_FILTER, "rotor");
    expect(f.hiddenGroups).toEqual(["rotor"]);
    f = toggleGroup(f, "rotor");
    expect(f.hiddenGroups).toEqual([]);
    for (const g of KIND_GROUPS) f = toggleGroup(f, g.id);
    expect(f.hiddenGroups).toHaveLength(KIND_GROUPS.length - 1);
  });

  it("counts the active filters", () => {
    expect(
      activeFilters(
        jetFilter({ altMinFt: 1, hiddenGroups: ["rotor"], militaryOnly: true, ground: "airborne" }),
      ),
    ).toBe(4);
  });
});
