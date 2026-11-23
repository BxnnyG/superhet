import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { chartLayout } from "./chartLayout";
import { formatDuration, minutesAt, routeProgress } from "./flightRoute";
import { parseHistory, parseOnline, parseOnlineAdmin } from "./online";
import { SAMPLE_STEP_MS, SeriesStore } from "./series";
import { squawkMeaning } from "./squawk";
import { historyRaw, onlineFullRaw, onlineRaw, onlineSettingsRaw } from "./testing";
import { Loader, SETTLE_MS } from "./useExtras";
import { parseStatus } from "./model";
import { adsbStatusRaw } from "./testing";

describe("parseOnline", () => {
  it("reads a full answer", () => {
    const r = parseOnline(onlineFullRaw());
    expect(r?.enabled).toBe(true);
    expect(r?.aircraft?.type).toBe("A350 941");
    expect(r?.route?.origin.iata).toBe("FRA");
    expect(r?.route?.destination.lat).toBeCloseTo(40.6398);
    expect(r?.route?.airline?.name).toBe("Lufthansa");
    expect(r?.photo?.photographer).toBe("Günther Feniuk");
    expect(r?.photo?.width).toBe(395);
  });

  it("reads the off state", () => {
    const r = parseOnline(onlineRaw());
    expect(r).toMatchObject({ enabled: false, adsbdb: "off", planespotters: "off", photo: null });
  });

  it("refuses a photo that is not on the photo service's hosts", () => {
    const photo = onlineFullRaw().photo as unknown as Record<string, unknown>;
    for (const bad of [
      { ...photo, src: "https://evil.example/x.jpg" },
      { ...photo, src: "http://t.plnspttrs.net/x.jpg" },
      { ...photo, src: "https://t.plnspttrs.net.evil.example/x.jpg" },
      { ...photo, src: "javascript:alert(1)" },
      { ...photo, link: "https://evil.example/photo/1" },
      { ...photo, photographer: "" },
    ]) {
      expect(parseOnline(onlineFullRaw({ photo: bad }))?.photo).toBeNull();
    }
  });

  it("is defensive about what the service made of it", () => {
    expect(parseOnline(null)).toBeNull();
    expect(parseOnline({ enabled: "yes" })).toBeNull();
    const r = parseOnline(
      onlineFullRaw({ route: { origin: { name: "A" } }, sources: { adsbdb: "weird" } }),
    );
    expect(r?.route).toBeNull();
    expect(r?.adsbdb).toBe("unavailable");
  });
});

describe("parseHistory / parseOnlineAdmin", () => {
  it("reads the history and null for an unknown aircraft", () => {
    expect(parseHistory(historyRaw())).toMatchObject({ events: 14, days: 3 });
    expect(parseHistory(historyRaw())?.callsigns.map((c) => c.callsign)).toEqual([
      "DLH123",
      "DLH456",
    ]);
    expect(parseHistory({ v: 1, seen: false })).toBeNull();
    expect(parseHistory(historyRaw({ events: "x" }))).toBeNull();
  });

  it("reads the admin settings", () => {
    const a = parseOnlineAdmin(onlineSettingsRaw({ settings: { enabled: true, adsbdb: true } }));
    expect(a?.settings).toEqual({ enabled: true, adsbdb: true, planespotters: false });
    expect(a?.sources.map((s) => s.id)).toEqual(["adsbdb", "planespotters"]);
    expect(parseOnlineAdmin({})).toBeNull();
  });
});

describe("routeProgress", () => {
  const fra = { lat: 50.03333, lon: 8.57056 };
  const jfk = { lat: 40.6398, lon: -73.7789 };

  it("is 0 at the origin, 1 at the destination and about half way in between", () => {
    expect(routeProgress(fra, jfk, fra)?.fraction).toBeCloseTo(0, 2);
    expect(routeProgress(fra, jfk, jfk)?.fraction).toBeCloseTo(1, 2);
    // The great circle's mid point: Frankfurt to New York passes near southern Greenland.
    const mid = routeProgress(fra, jfk, { lat: 59.5, lon: -34 });
    expect(mid?.plausible).toBe(true);
    expect(mid?.fraction).toBeGreaterThan(0.4);
    expect(mid?.fraction).toBeLessThan(0.6);
    expect(routeProgress(fra, jfk, fra)?.totalKm).toBeGreaterThan(6000);
  });

  it("flags a position far from the route and one behind the origin", () => {
    expect(routeProgress(fra, jfk, { lat: -25, lon: 135 })?.plausible).toBe(false);
    const behind = routeProgress(fra, jfk, { lat: 52, lon: 22 });
    expect(behind?.plausible).toBe(false);
    expect(behind?.doneKm).toBe(0);
  });

  it("has no progress when origin and destination are the same place", () => {
    expect(routeProgress(fra, fra, jfk)).toBeNull();
  });

  it("estimates the time to go only at a sensible speed", () => {
    expect(minutesAt(900, 486)).toBeCloseTo(60, 0);
    expect(minutesAt(900, 20)).toBeNull();
    expect(minutesAt(900, null)).toBeNull();
    expect(formatDuration(85)).toBe("1 h 25 min");
    expect(formatDuration(42)).toBe("42 min");
  });
});

describe("squawk", () => {
  it("knows the emergency codes and a few standing ones", () => {
    for (const code of ["7500", "7600", "7700"]) {
      expect(squawkMeaning(code)?.emergency).toBe(true);
    }
    expect(squawkMeaning("7000")).toMatchObject({ emergency: false });
    expect(squawkMeaning("1234")).toBeNull();
    expect(squawkMeaning(null)).toBeNull();
  });
});

describe("chartLayout", () => {
  const pts = (vs: (number | null)[]) => vs.map((v, i) => ({ t: 1000 + i * 1000, v }));

  it("needs two values and keeps a flat line off the edge", () => {
    expect(chartLayout(pts([5]), 2000, 60_000, false)).toBeNull();
    const flat = chartLayout(pts([5, 5, 5]), 3000, 60_000, false);
    expect(flat?.min).toBeLessThan(5);
    expect(flat?.max).toBeGreaterThan(5);
    expect(flat?.last).toBe(5);
  });

  it("breaks the line where a value is missing and keeps zero in view when asked", () => {
    const c = chartLayout(pts([100, 200, null, 300, 400]), 5000, 60_000, true);
    expect((c?.path.match(/M/g) ?? []).length).toBe(2);
    expect(c?.min).toBe(0);
    expect(c?.zeroY).not.toBeNull();
  });

  it("ignores values older than the window", () => {
    const c = chartLayout(pts([1, 2, 3, 4]), 3000 + 100_000, 60_000, false);
    expect(c).toBeNull();
  });
});

describe("SeriesStore", () => {
  const picture = (alt: number, icao = "3c6444") =>
    parseStatus(
      adsbStatusRaw({ aircraft: [{ icao, alt_ft: alt, speed_kt: 400, messages: 1, seen_s: 0 }] }),
    )?.aircraft ?? [];

  it("samples at most once per step, forgets vanished aircraft and keeps aircraft without a position", () => {
    const s = new SeriesStore();
    s.record(picture(1000), 0);
    s.record(picture(2000), SAMPLE_STEP_MS - 1);
    s.record(picture(3000), SAMPLE_STEP_MS);
    expect(s.get("3c6444").map((x) => x.altFt)).toEqual([1000, 3000]);
    s.record(picture(1, "406a3d"), 2 * SAMPLE_STEP_MS);
    expect(s.get("3c6444")).toEqual([]);
    expect(s.get("406a3d")).toHaveLength(1);
    s.clear();
    expect(s.get("406a3d")).toEqual([]);
  });
});

describe("Loader", () => {
  beforeEach(() => vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "Date"] }));
  afterEach(() => vi.useRealTimers());

  it("waits for the selection to settle, keeps answers and asks again for unkept ones", async () => {
    const loader = new Loader<number>((n) => n > 0);
    const fetcher = vi.fn<(signal: AbortSignal) => Promise<number>>(() => Promise.resolve(1));
    loader.request("a", fetcher);
    expect(loader.snapshot("a")).toEqual({ status: "loading" });
    expect(fetcher).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(SETTLE_MS + 1);
    expect(loader.snapshot("a")).toEqual({ status: "ready", data: 1 });
    // Kept: no second request.
    loader.request("a", fetcher);
    await vi.advanceTimersByTimeAsync(SETTLE_MS + 1);
    expect(fetcher).toHaveBeenCalledTimes(1);

    // A paused service's answer is shown but not kept.
    const paused = vi.fn<(signal: AbortSignal) => Promise<number>>(() => Promise.resolve(0));
    loader.request("b", paused);
    await vi.advanceTimersByTimeAsync(SETTLE_MS + 1);
    loader.request("b", paused);
    await vi.advanceTimersByTimeAsync(SETTLE_MS + 1);
    expect(paused).toHaveBeenCalledTimes(2);
  });

  it("cancels a request that was replaced before it started", async () => {
    const loader = new Loader<number>();
    const first = vi.fn<(signal: AbortSignal) => Promise<number>>(() => Promise.resolve(1));
    const cancel = loader.request("a", first);
    cancel();
    await vi.advanceTimersByTimeAsync(SETTLE_MS + 1);
    expect(first).not.toHaveBeenCalled();
  });

  it("reports an error and recovers on a forced retry", async () => {
    const loader = new Loader<number>();
    loader.request("a", () => Promise.reject(new Error("down")));
    await vi.advanceTimersByTimeAsync(SETTLE_MS + 1);
    expect(loader.snapshot("a").status).toBe("error");
    loader.request("a", () => Promise.resolve(7), true);
    await vi.advanceTimersByTimeAsync(SETTLE_MS + 1);
    expect(loader.snapshot("a")).toEqual({ status: "ready", data: 7 });
  });
});
