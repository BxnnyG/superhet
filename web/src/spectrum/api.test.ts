import { describe, expect, it, vi } from "vitest";
import { ApiError, createApiClient, parseDeviceState, parseTuneReply, tuneBody } from "./api";

const rawState = {
  v: 1,
  device: "dev0",
  driver: "rtlsdr",
  tuner: "R828D",
  center_hz: 100_000_000,
  sample_rate: 2_048_000,
  gain: { mode: "manual", db: 28 },
  gains_db: [49.6, 0, 0.9],
  ppm: 0,
  bias_tee: false,
  tuning_epoch: 3,
};

function response(body: unknown, status = 200): Response {
  return new Response(typeof body === "string" ? body : JSON.stringify(body), { status });
}

describe("parseDeviceState", () => {
  it("maps fields and sorts gains", () => {
    const s = parseDeviceState(rawState);
    expect(s).toMatchObject({
      driver: "rtlsdr",
      tuner: "R828D",
      centerHz: 100_000_000,
      sampleRate: 2_048_000,
      gain: { mode: "manual", db: 28 },
      gainsDb: [0, 0.9, 49.6],
      tuningEpoch: 3,
    });
  });

  it("reads the ADC overload report and tolerates its absence", () => {
    expect(parseDeviceState(rawState).overload).toBeUndefined();
    expect(
      parseDeviceState({ ...rawState, overload: { active: true, clipped_percent: 7.3 } }).overload,
    ).toEqual({ active: true, clippedPercent: 7.3 });
    expect(parseDeviceState({ ...rawState, overload: { active: "yes" } }).overload).toBeUndefined();
  });

  it("accepts gain null (file replay) and a missing tuner", () => {
    const s = parseDeviceState({ ...rawState, gain: null, tuner: undefined, gains_db: [] });
    expect(s.gain).toBeNull();
    expect(s.tuner).toBeUndefined();
  });

  it.each([
    ["non-object", 5],
    ["missing centre", { ...rawState, center_hz: undefined }],
    ["string rate", { ...rawState, sample_rate: "2048000" }],
    ["zero rate", { ...rawState, sample_rate: 0 }],
    ["bad gain mode", { ...rawState, gain: { mode: "x", db: 1 } }],
    ["bad gains list", { ...rawState, gains_db: [1, "2"] }],
    ["missing driver", { ...rawState, driver: undefined }],
  ])("rejects %s", (_name, raw) => {
    expect(() => parseDeviceState(raw)).toThrow(ApiError);
  });
});

describe("parseTuneReply", () => {
  it("returns the state of an ok reply", () => {
    expect(parseTuneReply({ v: 1, ok: true, state: rawState }).centerHz).toBe(100_000_000);
  });

  it("turns ok:false into an ApiError with code", () => {
    try {
      parseTuneReply({ v: 1, ok: false, code: "unsupported", error: "no gain" }, 400);
      expect.unreachable();
    } catch (e) {
      expect(e).toBeInstanceOf(ApiError);
      expect((e as ApiError).code).toBe("unsupported");
      expect((e as ApiError).message).toBe("no gain");
    }
  });

  it("rejects malformed replies", () => {
    expect(() => parseTuneReply({ v: 1 })).toThrow(ApiError);
    expect(() => parseTuneReply({ ok: true })).toThrow(ApiError);
  });
});

describe("client", () => {
  it("builds the tune body with snake_case fields", () => {
    expect(tuneBody({ centerHz: 1, sampleRate: 2, gain: "auto", ppm: 3, biasTee: true })).toEqual({
      v: 1,
      center_hz: 1,
      sample_rate: 2,
      gain: "auto",
      ppm: 3,
      bias_tee: true,
    });
    expect(tuneBody({ gain: 12.5 })).toEqual({ v: 1, gain: 12.5 });
  });

  it("GETs the state", async () => {
    const fetchImpl = vi.fn().mockResolvedValue(response(rawState));
    const state = await createApiClient(fetchImpl).getState("dev 0");
    expect(state.sampleRate).toBe(2_048_000);
    expect(fetchImpl.mock.calls[0]?.[0]).toBe("/api/v1/devices/dev%200/state");
  });

  it("POSTs JSON to tune", async () => {
    const fetchImpl = vi.fn().mockResolvedValue(response({ v: 1, ok: true, state: rawState }));
    await createApiClient(fetchImpl).tune("dev0", { centerHz: 7_100_000 });
    const [url, init] = fetchImpl.mock.calls[0] as [string, RequestInit];
    expect(url).toBe("/api/v1/devices/dev0/tune");
    expect(init.method).toBe("POST");
    expect(init.headers).toEqual({ "Content-Type": "application/json" });
    expect(JSON.parse(init.body as string)).toEqual({ v: 1, center_hz: 7_100_000 });
  });

  it("surfaces error replies, HTTP failures and network errors", async () => {
    const err = response({ v: 1, ok: false, code: "invalid", error: "bad centre" }, 400);
    await expect(
      createApiClient(vi.fn().mockResolvedValue(err)).tune("dev0", { centerHz: -1 }),
    ).rejects.toMatchObject({ message: "bad centre", code: "invalid", status: 400 });

    await expect(
      createApiClient(vi.fn().mockResolvedValue(response("oops", 502))).getState("dev0"),
    ).rejects.toMatchObject({ message: "HTTP 502", status: 502 });

    await expect(
      createApiClient(vi.fn().mockRejectedValue(new TypeError("offline"))).getState("dev0"),
    ).rejects.toMatchObject({ message: "offline" });
  });
});

describe("lock support", () => {
  it("parses locks and the usable window, with a fallback", () => {
    const s = parseDeviceState({
      ...rawState,
      locks: [{ kind: "user", id: "user", since_ms: 9 }, { nope: 1 }],
      window: { start_hz: 1, end_hz: 2 },
    });
    expect(s.locks).toEqual([{ kind: "user", id: "user", sinceMs: 9 }]);
    expect(s.window).toEqual({ startHz: 1, endHz: 2 });
    const d = parseDeviceState(rawState);
    expect(d.locks).toEqual([]);
    expect(d.window.endHz - d.window.startHz).toBeCloseTo(2_048_000 * 0.85);
  });

  it("carries the holders of a 409 locked reply", async () => {
    const body = {
      v: 1,
      ok: false,
      code: "locked",
      error: "tuning is locked by: user",
      holders: [{ kind: "user", id: "user", since_ms: 1 }],
    };
    const fetchImpl = vi.fn().mockResolvedValue(response(body, 409));
    const err = await createApiClient(fetchImpl)
      .tune("dev0", { centerHz: 1 })
      .catch((e) => e);
    expect(err).toBeInstanceOf(ApiError);
    expect(err).toMatchObject({ status: 409, code: "locked" });
    expect(err.holders).toEqual([{ kind: "user", id: "user", sinceMs: 1 }]);
  });

  it("uses POST and DELETE on the lock route", async () => {
    const reply = { v: 1, ok: true, state: { ...rawState, locks: [{ kind: "user", id: "user" }] } };
    const fetchImpl = vi.fn().mockImplementation(async () => response(reply));
    const api = createApiClient(fetchImpl);
    const s = await api.lock("dev0");
    await api.unlock("dev0");
    expect(s.locks).toHaveLength(1);
    expect(fetchImpl).toHaveBeenNthCalledWith(1, "/api/v1/devices/dev0/lock", { method: "POST" });
    expect(fetchImpl).toHaveBeenNthCalledWith(2, "/api/v1/devices/dev0/lock", {
      method: "DELETE",
    });
  });
});
