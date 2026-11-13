import { describe, expect, it } from "vitest";
import {
  groupOf,
  KIND_GROUPS,
  SYMBOL_KINDS,
  SYMBOL_LABEL,
  SYMBOL_SIZE,
  symbolKind,
  symbolSvg,
  type SymbolInput,
  type SymbolKind,
} from "./symbols";

const cases: [string, SymbolInput, SymbolKind][] = [
  // The type designator is the most exact source.
  ["Airbus A320 by designator", { typeCode: "A320", typeClass: "L2J", wake: "M" }, "narrow"],
  ["Boeing 737-800", { typeCode: "B738", typeClass: "L2J", wake: "M" }, "narrow"],
  ["Airbus A359", { typeCode: "A359", typeClass: "L2J", wake: "H" }, "wide"],
  ["Boeing 777-300ER, designator alone", { typeCode: "B77W" }, "wide"],
  ["Boeing 747-8", { typeCode: "B748", typeClass: "L4J", wake: "H" }, "quad"],
  ["Airbus A380", { typeCode: "A388" }, "quad"],
  ["Gulfstream G650, designator", { typeCode: "GLF6" }, "bizjet"],
  ["Citation, designator", { typeCode: "C25B" }, "bizjet"],
  ["Learjet", { typeCode: "LJ45" }, "bizjet"],
  ["Falcon 2000 is no fighter", { typeCode: "F2TH" }, "bizjet"],
  ["Fokker 100 is no fighter", { typeCode: "F100", typeClass: "L2J", wake: "M" }, "narrow"],
  ["Fokker 50 is a turboprop", { typeCode: "F50", typeClass: "L2T", wake: "M" }, "twinprop"],
  ["Eurofighter", { typeCode: "EUFI", typeClass: "L2J", wake: "M", military: true }, "fighter"],
  ["F-16", { typeCode: "F16" }, "fighter"],
  ["balloon designator", { typeCode: "BALL" }, "balloon"],
  ["glider designator", { typeCode: "GLID", typeClass: "L1P", wake: "L" }, "glider"],
  ["ground vehicle designator", { typeCode: "GRND" }, "ground"],
  ["obstacle designator", { typeCode: "OBST" }, "tower"],
  ["Reaper drone", { typeCode: "MQ9" }, "uav"],
  // ICAO type description (class, engines, engine type).
  ["twin jet, medium wake", { typeClass: "L2J", wake: "M" }, "narrow"],
  ["twin jet, heavy wake", { typeClass: "L2J", wake: "H" }, "wide"],
  ["twin jet, light wake", { typeClass: "L2J", wake: "L" }, "bizjet"],
  ["tri-jet, heavy", { typeClass: "L3J", wake: "H" }, "wide"],
  ["tri-jet, medium", { typeClass: "L3J", wake: "M" }, "narrow"],
  ["four jets", { typeClass: "L4J", wake: "M" }, "quad"],
  ["four turboprops", { typeClass: "L4T", wake: "H" }, "quad"],
  ["single jet, civil", { typeClass: "L1J", wake: "L" }, "bizjet"],
  ["single jet, military", { typeClass: "L1J", wake: "L", military: true }, "fighter"],
  ["single piston", { typeClass: "L1P", wake: "L" }, "light"],
  ["single turboprop", { typeClass: "L1T", wake: "L" }, "light"],
  ["twin piston", { typeClass: "L2P", wake: "L" }, "twinprop"],
  ["twin turboprop", { typeClass: "L2T", wake: "M" }, "twinprop"],
  ["amphibian", { typeClass: "A1P", wake: "L" }, "light"],
  ["helicopter", { typeClass: "H2T", wake: "M" }, "heli"],
  ["gyrocopter", { typeClass: "G1P", wake: "L" }, "heli"],
  ["tiltrotor", { typeClass: "T2T", wake: "M" }, "heli"],
  // Emitter category, when the database does not know the aircraft.
  ["A1 light", { category: "A1" }, "light"],
  ["A2 small", { category: "A2" }, "bizjet"],
  ["A3 large", { category: "A3" }, "narrow"],
  ["A4 high vortex", { category: "A4" }, "narrow"],
  ["A5 heavy", { category: "A5" }, "wide"],
  ["A6 high performance", { category: "A6" }, "fighter"],
  ["A7 rotorcraft", { category: "A7" }, "heli"],
  ["B1 glider", { category: "B1" }, "glider"],
  ["B2 lighter than air", { category: "B2" }, "balloon"],
  ["B4 ultralight", { category: "B4" }, "glider"],
  ["B6 UAV", { category: "B6" }, "uav"],
  ["C1 emergency vehicle", { category: "C1" }, "ground"],
  ["C2 service vehicle", { category: "C2" }, "ground"],
  ["C3 point obstacle", { category: "C3" }, "tower"],
  // The database beats the category.
  ["category says heavy, database says light prop", { typeClass: "L1P", category: "A5" }, "light"],
  // Last resorts and nothing at all.
  ["wake heavy only", { wake: "H" }, "wide"],
  ["wake medium only", { wake: "M" }, "narrow"],
  ["wake light only says nothing", { wake: "L" }, "unknown"],
  ["nothing known", {}, "unknown"],
  ["unrecognised bits", { typeCode: "ZZZZ", typeClass: "??", category: "A0" }, "unknown"],
  ["lower case input", { typeClass: "h1t", category: "a3" }, "heli"],
];

describe("symbolKind", () => {
  it.each(cases)("%s", (_name, input, kind) => {
    expect(symbolKind(input)).toBe(kind);
  });

  it("is never a jet for a plain light aircraft", () => {
    expect(symbolKind({ typeClass: "L1P", wake: "L", category: "A1" })).not.toBe("narrow");
  });
});

describe("the symbols", () => {
  it("have a label, a size and a drawing for every kind", () => {
    for (const kind of SYMBOL_KINDS) {
      expect(SYMBOL_LABEL[kind]).not.toBe("");
      expect(SYMBOL_SIZE[kind]).toBeGreaterThan(10);
      expect(symbolSvg(kind)).toMatch(/^<svg viewBox="0 0 32 32"[^>]*>.+<\/svg>$/);
    }
  });

  it("scale with the aircraft", () => {
    expect(SYMBOL_SIZE.quad).toBeGreaterThan(SYMBOL_SIZE.wide);
    expect(SYMBOL_SIZE.wide).toBeGreaterThan(SYMBOL_SIZE.narrow);
    expect(SYMBOL_SIZE.narrow).toBeGreaterThan(SYMBOL_SIZE.light);
  });

  it("are all different drawings", () => {
    const drawings = new Set(
      SYMBOL_KINDS.map((k) => symbolSvg(k).replace(/width="\d+" height="\d+"/, "")),
    );
    expect(drawings.size).toBe(SYMBOL_KINDS.length);
  });

  it("sit in exactly one filter group", () => {
    for (const kind of SYMBOL_KINDS) {
      const groups = KIND_GROUPS.filter((g) => (g.kinds as readonly string[]).includes(kind));
      expect(groups).toHaveLength(1);
      expect(groupOf(kind)).toBe(groups[0]?.id);
    }
  });
});
