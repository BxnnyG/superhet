import { describe, expect, it } from "vitest";
import { parseEvent, type DecoderEvent } from "../decoders/events";
import {
  aprsStations,
  DROP_AFTER_MS,
  heardAgo,
  MAX_TRACK,
  stationOpacity,
  STALE_AFTER_MS,
} from "./aprsStations";

let nextId = 1;
export const packet = (tsMs: number, data: Record<string, unknown>): DecoderEvent => {
  const e = parseEvent({
    id: nextId++,
    ts_ms: tsMs,
    device: "dev0",
    vfo: 1,
    freq_hz: 144_800_000,
    mode: null,
    kind: "aprs.packet",
    source: "org.superhet.decoder.aprs",
    value: "",
    meta: { instance: "i-1", ts: null, data: { dest: "APRS", path: [], info: "", ...data } },
  });
  if (!e) throw new Error("fixture");
  return e;
};

const NOW = 1_700_000_000_000;

describe("aprsStations", () => {
  it("keeps the last position and builds the track, whatever the event order", () => {
    const events = [
      packet(NOW - 1000, { source: "N0CALL-9", lat: 52.6, lon: 13.5, symbol: "/>", speed_kn: 30 }),
      packet(NOW - 3000, { source: "N0CALL-9", lat: 52.5, lon: 13.4, symbol: "/>", comment: "hi" }),
      packet(NOW - 2000, { source: "N0CALL-9", lat: 52.5, lon: 13.4 }),
    ];
    const [s, ...rest] = aprsStations(events, NOW);
    expect(rest).toEqual([]);
    expect(s?.id).toBe("N0CALL-9");
    expect([s?.lat, s?.lon]).toEqual([52.6, 13.5]);
    expect(s?.symbol).toBe("/>");
    expect(s?.speedKn).toBe(30);
    expect(s?.packets).toBe(3);
    // The repeated position adds no track point.
    expect(s?.track.map((p) => [p.lat, p.lon])).toEqual([
      [52.5, 13.4],
      [52.6, 13.5],
    ]);
  });

  it("ignores packets without a valid position", () => {
    const bad = [
      packet(NOW, { source: "A1", type: "message" }),
      packet(NOW, { source: "A2", lat: 95, lon: 10 }),
      packet(NOW, { lat: 50, lon: 10 }),
    ];
    expect(aprsStations(bad, NOW)).toEqual([]);
  });

  it("names objects by their object name and remembers the sender", () => {
    const [s] = aprsStations(
      [packet(NOW, { source: "N0CALL", name: "LEADER", lat: 1, lon: 2, live: false })],
      NOW,
    );
    expect(s?.id).toBe("LEADER");
    expect(s?.via).toBe("N0CALL");
    expect(s?.live).toBe(false);
  });

  it("caps the track, sorts by last heard and drops long-silent stations", () => {
    const many = Array.from({ length: MAX_TRACK + 10 }, (_, i) =>
      packet(NOW - 100_000 + i, { source: "MOVER", lat: 50 + i / 1000, lon: 8 }),
    );
    const events = [
      ...many,
      packet(NOW - 10, { source: "RECENT", lat: 51, lon: 9 }),
      packet(NOW - DROP_AFTER_MS - 1, { source: "GONE", lat: 40, lon: 9 }),
    ];
    const list = aprsStations(events, NOW);
    expect(list.map((s) => s.id)).toEqual(["RECENT", "MOVER"]);
    expect(list[1]?.track).toHaveLength(MAX_TRACK);
  });

  it("fades stale stations and words the age", () => {
    const [fresh] = aprsStations([packet(NOW - 5000, { source: "A", lat: 1, lon: 1 })], NOW);
    const [stale] = aprsStations(
      [packet(NOW - STALE_AFTER_MS - 1, { source: "B", lat: 1, lon: 1 })],
      NOW,
    );
    if (!fresh || !stale) throw new Error("fixture");
    expect(stationOpacity(fresh, NOW)).toBe(1);
    expect(stationOpacity(stale, NOW)).toBeLessThan(1);
    expect(heardAgo(fresh, NOW)).toBe("just now");
    expect(heardAgo(stale, NOW)).toBe("30 min ago");
  });
});
