import { describe, expect, it, vi } from "vitest";
import { ApiError } from "../spectrum/api";
import {
  createBody,
  createVfoApi,
  parseListReply,
  parseVfo,
  parseVfoReply,
  patchBody,
} from "./api";

const raw = {
  id: 2,
  freq_hz: 100_000_000,
  mode: "wfm",
  bandwidth_hz: 180_000,
  squelch_dbfs: null,
  deemphasis: "50us",
  offset_hz: 0,
  status: "active",
};

describe("parsing", () => {
  it("parses a VFO object", () => {
    expect(parseVfo(raw)).toEqual({
      id: 2,
      freqHz: 100_000_000,
      mode: "wfm",
      bandwidthHz: 180_000,
      squelchDbfs: null,
      deemphasis: "50us",
      offsetHz: 0,
      status: "active",
    });
    expect(parseVfo({ ...raw, squelch_dbfs: -70, status: "paused" })).toMatchObject({
      squelchDbfs: -70,
      status: "paused",
    });
  });

  it.each([{ id: 0 }, { mode: "usb" }, { freq_hz: "x" }, { bandwidth_hz: null }])(
    "rejects %j",
    (bad) => {
      expect(() => parseVfo({ ...raw, ...bad })).toThrow(ApiError);
    },
  );

  it("parses lists sorted by id", () => {
    const list = parseListReply({ v: 1, vfos: [{ ...raw, id: 3 }, raw] });
    expect(list.map((v) => v.id)).toEqual([2, 3]);
    expect(parseListReply({ v: 1 })).toEqual([]);
  });

  it("parses replies with and without state", () => {
    expect(parseVfoReply({ ok: true, vfo: raw }).vfos).toBeNull();
    const r = parseVfoReply({ ok: true, vfo: raw, state: { vfos: [raw] } }, 201);
    expect(r.vfo.id).toBe(2);
    expect(r.vfos).toHaveLength(1);
  });

  it("turns ok:false into an ApiError with code", () => {
    try {
      parseVfoReply({ ok: false, code: "unsupported", error: "limit of 8 VFOs" }, 400);
      expect.unreachable();
    } catch (e) {
      expect(e).toBeInstanceOf(ApiError);
      expect((e as ApiError).code).toBe("unsupported");
      expect((e as ApiError).message).toBe("limit of 8 VFOs");
    }
  });
});

describe("bodies", () => {
  it("maps create fields", () => {
    expect(createBody({ freqHz: 1, mode: "am" })).toEqual({ v: 1, freq_hz: 1, mode: "am" });
    expect(createBody({ freqHz: 1, mode: "am", bandwidthHz: 5000, squelchDbfs: -80 })).toEqual({
      v: 1,
      freq_hz: 1,
      mode: "am",
      bandwidth_hz: 5000,
      squelch_dbfs: -80,
    });
  });

  it("keeps null (reset) in patches and omits undefined", () => {
    expect(patchBody({ bandwidthHz: null, squelchDbfs: null })).toEqual({
      v: 1,
      bandwidth_hz: null,
      squelch_dbfs: null,
    });
    expect(patchBody({ freqHz: 5 })).toEqual({ v: 1, freq_hz: 5 });
  });
});

describe("createVfoApi", () => {
  const json = (body: unknown, status = 200) =>
    new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });

  it("calls the REST routes", async () => {
    const fetchImpl = vi.fn((_url: string, init?: RequestInit) => {
      if (init?.method === "DELETE") return Promise.resolve(new Response(null, { status: 204 }));
      if (init?.method === "POST") return Promise.resolve(json({ ok: true, vfo: raw }, 201));
      if (init?.method === "PATCH") return Promise.resolve(json({ ok: true, vfo: raw }));
      return Promise.resolve(json({ v: 1, vfos: [raw] }));
    });
    const api = createVfoApi(fetchImpl as unknown as typeof fetch);
    expect(await api.list("dev0")).toHaveLength(1);
    await api.create("dev0", { freqHz: 1, mode: "nfm" });
    await api.update("dev0", 2, { freqHz: 5 });
    await api.remove("dev0", 2);
    const calls = fetchImpl.mock.calls.map(([u, i]) => `${i?.method ?? "GET"} ${u}`);
    expect(calls).toEqual([
      "GET /api/v1/devices/dev0/vfos",
      "POST /api/v1/devices/dev0/vfos",
      "PATCH /api/v1/devices/dev0/vfos/2",
      "DELETE /api/v1/devices/dev0/vfos/2",
    ]);
    expect(JSON.parse(String(fetchImpl.mock.calls[2]?.[1]?.body))).toEqual({ v: 1, freq_hz: 5 });
  });

  it("surfaces HTTP errors", async () => {
    const fetchImpl = vi.fn(() =>
      Promise.resolve(json({ code: "not_found", error: "no vfo" }, 404)),
    );
    const api = createVfoApi(fetchImpl as unknown as typeof fetch);
    await expect(api.update("dev0", 9, { freqHz: 1 })).rejects.toMatchObject({
      status: 404,
      code: "not_found",
    });
    fetchImpl.mockImplementationOnce(() => Promise.reject(new Error("offline")));
    await expect(api.remove("dev0", 1)).rejects.toThrow("offline");
  });
});
