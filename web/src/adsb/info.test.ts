import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { InfoCache, MISS_TTL_MS, parseDatabaseStatus, parseLookup } from "./info";
import { databaseStatusRaw, lookupRaw } from "./testing";

describe("parseLookup", () => {
  it("reads aircraft and airlines", () => {
    const r = parseLookup(lookupRaw());
    expect(r.aircraft.get("3c6444")).toEqual({
      registration: "D-AIXA",
      typeCode: "A359",
      typeDesc: "AIRBUS A-350-900",
      typeClass: "L2J",
      wake: "H",
      military: false,
    });
    expect(r.operators.get("DLH")).toEqual({
      name: "Lufthansa",
      country: "Germany",
      callsign: "LUFTHANSA",
    });
  });

  it("survives junk", () => {
    expect(parseLookup(null).aircraft.size).toBe(0);
    expect(
      parseLookup({ aircraft: { "3c6444": 5 }, operators: { X: { name: "" } } }).aircraft.size,
    ).toBe(0);
    const r = parseLookup({ aircraft: { "3C6444": { reg: "", type: "A320" } } });
    expect(r.aircraft.get("3c6444")?.registration).toBeNull();
  });
});

describe("parseDatabaseStatus", () => {
  it("reads the source, the offer and the job", () => {
    const s = parseDatabaseStatus(databaseStatusRaw());
    expect(s?.database?.licence).toBe("ODC-By 1.0");
    expect(s?.database?.aircraft).toBe(455928);
    expect(s?.offer.archive).toBe(true);
    expect(s?.job.state).toBe("idle");
    expect(parseDatabaseStatus(databaseStatusRaw({ database: null }))?.database).toBeNull();
    expect(
      parseDatabaseStatus(databaseStatusRaw({ job: { state: "bogus", message: "x" } }))?.job.state,
    ).toBe("idle");
    expect(parseDatabaseStatus({})).toBeNull();
  });
});

describe("InfoCache", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(1_790_000_000_000);
  });
  afterEach(() => vi.useRealTimers());

  const make = () => {
    const lookup = vi.fn(() => Promise.resolve(parseLookup(lookupRaw())));
    return { lookup, cache: new InfoCache({ lookup }, 100) };
  };

  it("batches what is asked within the debounce window and answers from the cache", async () => {
    const { lookup, cache } = make();
    const seen = vi.fn();
    cache.subscribe(seen);
    cache.request(["3c6444"], ["DLH"]);
    cache.request(["406a3d", "3c6444", "nothex"], []);
    expect(lookup).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(100);
    expect(lookup).toHaveBeenCalledTimes(1);
    expect(lookup.mock.calls[0]).toEqual([["3c6444", "406a3d"], ["DLH"]]);
    expect(cache.aircraftInfo("3c6444")?.registration).toBe("D-AIXA");
    expect(cache.operatorInfo("DLH")?.name).toBe("Lufthansa");
    expect(seen).toHaveBeenCalled();
    expect(cache.getSnapshot()).toBeGreaterThan(0);

    // Known now: no new request.
    cache.request(["3c6444", "406a3d"], ["DLH"]);
    await vi.advanceTimersByTimeAsync(500);
    expect(lookup).toHaveBeenCalledTimes(1);
  });

  it("remembers misses for a while and then asks again", async () => {
    const { lookup, cache } = make();
    cache.request(["123456"], []);
    await vi.advanceTimersByTimeAsync(100);
    expect(cache.aircraftInfo("123456")).toBeNull();
    cache.request(["123456"], []);
    await vi.advanceTimersByTimeAsync(200);
    expect(lookup).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(MISS_TTL_MS);
    cache.request(["123456"], []);
    await vi.advanceTimersByTimeAsync(200);
    expect(lookup).toHaveBeenCalledTimes(2);
  });

  it("splits large requests and stays quiet after a failure", async () => {
    const lookup = vi.fn(() => Promise.reject(new Error("down")));
    const cache = new InfoCache({ lookup }, 10);
    const many = Array.from({ length: 250 }, (_, i) => i.toString(16).padStart(6, "0"));
    cache.request(many, []);
    await vi.advanceTimersByTimeAsync(10);
    expect(lookup).toHaveBeenCalledTimes(1);
    // Within the quiet period nothing is asked.
    cache.request(["abcdef"], []);
    await vi.advanceTimersByTimeAsync(100);
    expect(lookup).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(31_000);
    cache.request(["abcdef"], []);
    await vi.advanceTimersByTimeAsync(100);
    expect(lookup).toHaveBeenCalledTimes(2);

    const ok = vi.fn(() => Promise.resolve(parseLookup({})));
    const big = new InfoCache({ lookup: ok }, 10);
    big.request(many, []);
    await vi.advanceTimersByTimeAsync(10);
    expect(ok).toHaveBeenCalledTimes(3);
  });

  it("forgets everything on clear", async () => {
    const { cache } = make();
    cache.request(["3c6444"], []);
    await vi.advanceTimersByTimeAsync(100);
    cache.clear();
    expect(cache.aircraftInfo("3c6444")).toBeNull();
  });
});
