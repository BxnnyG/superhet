import { describe, expect, it } from "vitest";
import {
  STALE_AFTER_S,
  ageOf,
  altitudeColor,
  fadeOf,
  formatAlt,
  formatTrack,
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
    expect(formatAlt(dlh)).toBe("35,000 ft");
    expect(formatAlt(ezy)).toBe("ground");
    expect(formatTrack(dlh)).toBe("124°");
    expect(altitudeColor(ezy)).not.toBe(altitudeColor(dlh));
    expect(altitudeColor({ ...dlh, altFt: 3000 })).not.toBe(altitudeColor(dlh));
  });
});
