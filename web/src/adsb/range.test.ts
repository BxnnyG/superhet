import { describe, expect, it } from "vitest";
import {
  bandMax,
  compass,
  destination,
  farthest,
  outline,
  parseRange,
  ringStep,
  sectorMax,
  SECTORS,
} from "./range";
import { rangeRaw } from "./testing";

const data = () => {
  const parsed = parseRange(rangeRaw());
  if (parsed === null) throw new Error("fixture");
  return parsed;
};

describe("parseRange", () => {
  it("reads cells, series and the receiver", () => {
    const d = data();
    expect(d.receiver).toEqual({ lat: 51, lon: 7 });
    expect(d.cells).toHaveLength(4);
    expect(d.series[1]).toEqual({
      tsMs: 1_790_000_000_000,
      msgsPerS: 140,
      aircraft: 12,
      withPosition: 9,
    });
    expect(d.bucketS).toBe(120);
  });

  it("drops cells outside the grid and survives junk", () => {
    const d = parseRange(
      rangeRaw({
        cells: [
          [72, 0, 1, ""],
          [0, 6, 1, ""],
          [0, 0, -1, ""],
          [1.5, 0, 1, ""],
          "x",
          [3, 1, 50, "abc"],
        ],
      }),
    );
    expect(d?.cells).toEqual([{ sector: 3, band: 1, km: 50, icao: "abc" }]);
    expect(parseRange({ v: 2 })).toBeNull();
    expect(parseRange(null)).toBeNull();
    expect(parseRange(rangeRaw({ receiver: null }))?.receiver).toBeNull();
  });
});

describe("sector and band maxima", () => {
  it("takes the farthest per sector, optionally for one band", () => {
    const d = data();
    const all = sectorMax(d.cells, null);
    expect(all).toHaveLength(SECTORS);
    expect(all[0]).toBe(180.5);
    expect(all[18]).toBe(210);
    expect(all[40]).toBe(12.5);
    expect(all[1]).toBe(0);
    expect(sectorMax(d.cells, 2)[18]).toBe(95.25);
    expect(sectorMax(d.cells, 3).every((v) => v === 0)).toBe(true);
  });

  it("takes the farthest per band and overall", () => {
    const d = data();
    expect(bandMax(d.cells)).toEqual([12.5, 0, 95.25, 0, 210, 0]);
    expect(farthest(d.cells)).toMatchObject({ km: 210, sector: 18, icao: "3c6445" });
    expect(farthest([])).toBeNull();
  });
});

describe("geometry", () => {
  const home = { lat: 51, lon: 7 };

  it("walks along a bearing", () => {
    const [lat, lon] = destination(home, 0, 111.19);
    expect(lat).toBeCloseTo(52, 1);
    expect(lon).toBeCloseTo(7, 3);
    const [lat2, lon2] = destination(home, 90, 100);
    expect(lat2).toBeCloseTo(51, 1);
    expect(lon2).toBeGreaterThan(8);
    // Wraps around the date line.
    expect(destination({ lat: 0, lon: 179.5 }, 90, 111)[1]).toBeLessThan(-179);
  });

  it("builds one outline point per sector, the receiver where nothing was heard", () => {
    const maxima = sectorMax(data().cells, null);
    const poly = outline(home, maxima);
    expect(poly).toHaveLength(SECTORS);
    expect(poly[1]).toEqual([51, 7]);
    const north = poly[0];
    expect(north?.[0]).toBeGreaterThan(52.5);
  });

  it("names directions and picks round ring steps", () => {
    expect(compass(0)).toBe("N");
    expect(compass(18)).toBe("E");
    expect(compass(36)).toBe("S");
    expect(compass(54)).toBe("W");
    expect(compass(71)).toBe("N");
    expect(ringStep(210)).toBe(100);
    expect(ringStep(30)).toBe(10);
    expect(ringStep(0)).toBe(5);
    expect(ringStep(100000)).toBe(1000);
  });
});
