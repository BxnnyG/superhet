import { describe, expect, it } from "vitest";
import { countryOf, flagEmoji, operatorCode, photoLinks } from "./icao";
import { ICAO_RANGES } from "./icaoRanges";

describe("countryOf", () => {
  it("finds the state of an ICAO address block", () => {
    expect(countryOf("3c6444")).toEqual({ code: "de", name: "Germany" });
    expect(countryOf("A835AF")?.code).toBe("us");
    expect(countryOf("406a3d")?.code).toBe("gb");
    expect(countryOf("4840d6")?.code).toBe("nl");
    expect(countryOf("7c6b2d")?.code).toBe("au");
  });

  it("prefers the small block inside a larger one", () => {
    // Bermuda's block lies inside the United Kingdom's.
    expect(countryOf("400100")?.code).toBe("bm");
    expect(countryOf("406a3d")?.code).toBe("gb");
  });

  it("knows nothing about non-ICAO addresses and unallocated blocks", () => {
    expect(countryOf("~a1b2c3")).toBeNull();
    expect(countryOf("zzzzzz")).toBeNull();
    expect(countryOf("3c64")).toBeNull();
    expect(countryOf("ffffff")).toBeNull();
  });

  it("has a well-formed table", () => {
    expect(ICAO_RANGES.length).toBeGreaterThan(150);
    for (const [start, end, code, name] of ICAO_RANGES) {
      expect(start).toBeLessThanOrEqual(end);
      expect(end).toBeLessThanOrEqual(0xffffff);
      expect(code === "" || /^[a-z]{2}$/.test(code)).toBe(true);
      expect(name).not.toBe("");
    }
  });
});

describe("flagEmoji", () => {
  it("builds regional indicator pairs", () => {
    expect(flagEmoji("de")).toBe("\u{1F1E9}\u{1F1EA}");
    expect(flagEmoji("")).toBe("");
    expect(flagEmoji("xyz")).toBe("");
  });
});

describe("operatorCode", () => {
  it("takes the airline designator of a flight callsign", () => {
    expect(operatorCode("DLH123")).toBe("DLH");
    expect(operatorCode("baw9ab")).toBe("BAW");
    expect(operatorCode("RYR8QK")).toBe("RYR");
  });

  it("gives nothing for registrations and callsigns without a flight number", () => {
    expect(operatorCode("DAIXA")).toBeNull();
    expect(operatorCode("N123AB")).toBeNull();
    expect(operatorCode("REACH")).toBeNull();
    expect(operatorCode(null)).toBeNull();
  });
});

describe("photoLinks", () => {
  it("links to search pages by ICAO address and registration", () => {
    const links = photoLinks("3c6444", "D-AIXA");
    expect(links.map((l) => l.url)).toEqual([
      "https://www.planespotters.net/hex/3C6444",
      "https://www.jetphotos.com/registration/D-AIXA",
    ]);
  });

  it("only offers what it can build safely", () => {
    expect(photoLinks("3c6444", null)).toHaveLength(1);
    expect(photoLinks("3c6444", "a/b?c")).toHaveLength(1);
    expect(photoLinks("~3c6444", "D-AIXA")).toEqual([]);
  });
});
