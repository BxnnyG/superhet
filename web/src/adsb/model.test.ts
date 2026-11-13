import { describe, expect, it } from "vitest";
import {
  STALE_AFTER_S,
  ageOf,
  altitudeColor,
  altitudeLegend,
  emergencyOf,
  fadeOf,
  formatAlt,
  formatDistance,
  formatGeomAlt,
  formatMach,
  formatSpeed,
  formatTrack,
  formatVrate,
  vrateArrow,
  matchesFilter,
  parseReply,
  parseStatus,
  sortAircraft,
  tracked,
} from "./model";
import { adsbReplyRaw, adsbStatusRaw, aircraftRaw } from "./testing";

describe("parseStatus", () => {
  it("reads the aircraft, the receiver and the counters", () => {
    const status = parseStatus(adsbStatusRaw());
    expect(status?.mode).toBe("running");
    expect(status?.receiver).toEqual({ lat: 51, lon: 7 });
    expect(status?.messagesPerS).toBe(565);
    expect(status?.aircraft).toHaveLength(3);
    const [dlh, ezy, bare] = status?.aircraft ?? [];
    expect(dlh).toMatchObject({ icao: "3c6444", callsign: "DLH123", altFt: 35000, lat: 51.12345 });
    expect(ezy).toMatchObject({ onGround: true, altFt: null });
    expect(bare).toMatchObject({ callsign: null, lat: null, lon: null, seenS: 3.1 });
  });

  it("drops entries without an address and positions outside the globe", () => {
    const status = parseStatus(
      adsbStatusRaw({
        aircraft: [
          aircraftRaw({ icao: "" }),
          { callsign: "NOADDR" },
          aircraftRaw({ icao: "ABCDEF", lat: 95, lon: 7 }),
        ],
      }),
    );
    expect(status?.aircraft).toHaveLength(1);
    expect(status?.aircraft[0]).toMatchObject({ icao: "abcdef", lat: null, lon: null });
  });

  it("is null for a document of another version or none at all", () => {
    expect(parseStatus(null)).toBeNull();
    expect(parseStatus({ v: 2 })).toBeNull();
  });

  it("falls back to a starting plugin with defaults", () => {
    const status = parseStatus({ v: 1 });
    expect(status).toMatchObject({ mode: "starting", receiver: null, total: 0, aircraft: [] });
  });
});

describe("parseReply", () => {
  it("keeps the instance state and the status", () => {
    const reply = parseReply(adsbReplyRaw());
    expect(reply).toMatchObject({
      pluginId: "org.superhet.decoder.adsb",
      state: "running",
      freqHz: 1090000000,
    });
    expect(reply.status?.total).toBe(3);
  });

  it("rejects replies that are not a status", () => {
    expect(() => parseReply({})).toThrow();
  });
});

describe("ageing", () => {
  it("adds the time since the status arrived", () => {
    const a = parseStatus(adsbStatusRaw())?.aircraft[0];
    if (!a) throw new Error("fixture");
    expect(ageOf(a, 1_000, 1_000)).toBeCloseTo(0.2);
    expect(ageOf(a, 1_000, 11_000)).toBeCloseTo(10.2);
    // A clock that went backwards does not make aircraft younger.
    expect(ageOf(a, 5_000, 1_000)).toBeCloseTo(0.2);
  });

  it("fades from full strength at 15 s to a ghost at 60 s and is gone after", () => {
    expect(fadeOf(0)).toBe(1);
    expect(fadeOf(15)).toBe(1);
    expect(fadeOf(37.5)).toBeCloseTo(0.63, 2);
    expect(fadeOf(59.9)).toBeGreaterThanOrEqual(0.25);
    expect(fadeOf(59.9)).toBeLessThan(0.3);
    expect(fadeOf(STALE_AFTER_S)).toBe(0);
    expect(fadeOf(200)).toBe(0);
  });

  it("leaves aircraft off the picture after 60 s", () => {
    const status = parseStatus(adsbStatusRaw());
    if (!status) throw new Error("fixture");
    const at = 1_000_000;
    expect(tracked(status, at, at)).toHaveLength(3);
    // 58 s later the freshest (0.2 s old) is 58.2 s old: still there, faint; the 3.1 s one is gone.
    const late = tracked(status, at, at + 58_000);
    expect(late.map((a) => a.icao)).toEqual(["3c6444", "406a3d"]);
    expect(late[0]?.opacity).toBeLessThan(0.35);
    expect(tracked(status, at, at + 61_000)).toHaveLength(0);
  });
});

describe("table helpers", () => {
  const status = parseStatus(adsbStatusRaw());
  const list = tracked(status ?? ({ aircraft: [] } as never), 0, 0);

  it("sorts with unknown values last in both directions", () => {
    expect(sortAircraft(list, "alt", "desc").map((a) => a.icao)).toEqual([
      "3c6444",
      "406a3d", // on the ground counts as 0 ft
      "4ca7b2", // unknown
    ]);
    expect(sortAircraft(list, "alt", "asc").map((a) => a.icao)).toEqual([
      "406a3d",
      "3c6444",
      "4ca7b2",
    ]);
    expect(sortAircraft(list, "callsign", "asc").map((a) => a.icao)).toEqual([
      "3c6444",
      "406a3d",
      "4ca7b2",
    ]);
    expect(sortAircraft(list, "age", "asc")[0]?.icao).toBe("3c6444");
  });

  it("filters on callsign, address and squawk", () => {
    const [dlh, ezy] = list;
    if (!dlh || !ezy) throw new Error("fixture");
    expect(matchesFilter(dlh, "")).toBe(true);
    expect(matchesFilter(dlh, " dlh")).toBe(true);
    expect(matchesFilter(dlh, "3C64")).toBe(true);
    expect(matchesFilter(dlh, "1000")).toBe(true);
    expect(matchesFilter(ezy, "dlh")).toBe(false);
  });

  it("formats numbers and bands for the view", () => {
    const [dlh, ezy] = list;
    if (!dlh || !ezy) throw new Error("fixture");
    expect(formatAlt(dlh, "aviation").text).toBe("35,000 ft");
    expect(formatAlt(ezy, "aviation").text).toBe("ground");
    expect(formatTrack(dlh.trackDeg)).toBe("124°");
    expect(altitudeColor(ezy)).not.toBe(altitudeColor(dlh));
    expect(altitudeColor({ ...dlh, altFt: 3000 })).not.toBe(altitudeColor(dlh));
  });
});

describe("units in the view helpers", () => {
  const status = parseStatus(adsbStatusRaw());
  const [dlh] = tracked(status ?? ({ aircraft: [] } as never), 0, 0);
  if (!dlh) throw new Error("fixture");

  it("writes altitude, speed and climb rate in the chosen system with the other one as tooltip", () => {
    expect(formatAlt(dlh, "metric")).toEqual({ text: "10,668 m", other: "35,000 ft" });
    expect(formatSpeed(dlh.speedKt, "metric")).toEqual({ text: "836 km/h", other: "451 kt" });
    expect(formatSpeed(dlh.speedKt, "aviation").text).toBe("451 kt");
    expect(formatVrate(dlh, "metric")).toEqual({ text: "\u22120.3 m/s", other: "\u221264 ft/min" });
    // Mixed: feet and feet per minute like a flight deck, speed and distance metric.
    expect(formatAlt(dlh, "mixed").text).toBe("35,000 ft");
    expect(formatSpeed(dlh.speedKt, "mixed").text).toBe("836 km/h");
    expect(formatVrate(dlh, "mixed").text).toBe("\u221264 ft/min");
    expect(formatSpeed(null, "metric").text).toBe("\u2013");
  });

  it("measures distance and bearing from the receiver", () => {
    // Receiver at 51.0 / 7.0, aircraft at 51.12345 / 6.98765: about 14 km north.
    expect(dlh.distKm).toBeGreaterThan(13);
    expect(dlh.distKm).toBeLessThan(15);
    expect(dlh.bearing).toBeGreaterThan(340);
    expect(formatDistance(dlh.distKm, "metric").text).toMatch(/^1[34]\.\d km$/);
    expect(formatDistance(dlh.distKm, "aviation").text).toMatch(/^7\.\d NM$/);
    expect(formatDistance(null, "metric").text).toBe("\u2013");
  });

  it("sorts by distance, aircraft without a position last", () => {
    const list = tracked(status ?? ({ aircraft: [] } as never), 0, 0);
    const asc = sortAircraft(list, "distance", "asc").map((a) => a.icao);
    expect(asc[2]).toBe("4ca7b2");
    expect(asc[0]).toBe("3c6444");
    expect(sortAircraft(list, "distance", "desc").map((a) => a.icao)[2]).toBe("4ca7b2");
  });

  it("reads the extra fields and falls back to the geometric climb rate", () => {
    const parsed = parseStatus(
      adsbStatusRaw({
        aircraft: [
          aircraftRaw({
            vrate_fpm: undefined,
            geom_rate_fpm: 640,
            alt_geom_ft: 35525,
            ias_kt: 281,
            tas_kt: 468,
            mach: 0.812,
            heading_deg: 124.8,
          }),
        ],
      }),
    );
    const a = parsed?.aircraft[0];
    expect(a).toMatchObject({ altGeomFt: 35525, iasKt: 281, tasKt: 468, mach: 0.812 });
    expect(a?.vrateFpm).toBe(640);
    expect(a?.headingDeg).toBe(124.8);
    if (!a) throw new Error("fixture");
    expect(formatGeomAlt(a, "aviation").text).toBe("35,525 ft");
    expect(formatMach(a.mach)).toBe("M 0.812");
    expect(formatMach(null)).toBe("\u2013");
  });

  it("points an arrow along the vertical rate", () => {
    expect(vrateArrow(1500)).toBe("\u2191");
    expect(vrateArrow(-1500)).toBe("\u2193");
    expect(vrateArrow(40)).toBe("\u2192");
    expect(vrateArrow(null)).toBe("");
  });

  it("labels the altitude colours in the chosen unit", () => {
    expect(altitudeLegend("aviation").map(([, t]) => t)).toContain("< 5,000 ft");
    expect(altitudeLegend("metric").map(([, t]) => t)).toContain("< 1,524 m");
  });
});

describe("emergency squawks", () => {
  it.each([
    ["7500", "Unlawful interference (hijack)"],
    ["7600", "Radio failure"],
    ["7700", "General emergency"],
  ])("%s means %s", (squawk, meaning) => {
    expect(emergencyOf({ squawk })).toBe(meaning);
  });

  it("ignores every other code", () => {
    for (const squawk of ["1000", "7000", "7777", "2000", null]) {
      expect(emergencyOf({ squawk })).toBeNull();
    }
  });
});
