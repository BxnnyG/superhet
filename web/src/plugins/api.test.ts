import { describe, expect, it, vi } from "vitest";
import { ApiError } from "../spectrum/api";
import { createPluginApi, parseInstance, parseManifest, startBody } from "./api";
import { POCSAG_ID, POCSAG_RAW, instanceRaw } from "./testing";

const reply = (status: number, body?: unknown) =>
  new Response(body === undefined ? null : JSON.stringify(body), { status });

function client(res: Response) {
  const f = vi.fn(() => Promise.resolve(res.clone()));
  return { f, api: createPluginApi(f as unknown as typeof fetch) };
}

const sent = (f: ReturnType<typeof vi.fn>, n = 0) => {
  const [url, init] = f.mock.calls[n] as unknown as [string, RequestInit];
  return {
    url,
    method: init.method,
    body: init.body ? (JSON.parse(String(init.body)) as unknown) : undefined,
  };
};

describe("parsing", () => {
  it("reads a manifest", () => {
    const m = parseManifest(POCSAG_RAW);
    expect(m).toMatchObject({
      id: POCSAG_ID,
      name: "POCSAG / FLEX pagers",
      version: "0.1.0",
      license: "AGPL-3.0-only",
      type: "decoder",
      transport: "simple",
      runtimeKind: "process",
      input: { kind: "discriminator", sampleRate: 22050, format: "s16le", bandwidthHz: 12500 },
      device: { exclusive: false, centerHz: null },
      acknowledge: { text: expect.stringContaining("private"), url: expect.any(String) },
    });
  });

  it("rejects a manifest without an id", () => {
    expect(() => parseManifest({ plugin: { name: "x" } })).toThrow(ApiError);
    expect(() => parseManifest(null)).toThrow(ApiError);
  });

  it("reads an instance and treats an unknown state as failed", () => {
    expect(parseInstance(instanceRaw())).toMatchObject({
      id: "i-1",
      pluginId: POCSAG_ID,
      device: "dev0",
      vfo: 1,
      state: "running",
      inputDropped: 0,
    });
    expect(parseInstance(instanceRaw({ state: "weird" })).state).toBe("error");
    // Liveness counters; hosts that do not send them read as zero / unknown.
    expect(parseInstance(instanceRaw())).toMatchObject({
      inputBytes: 441_000,
      inputLevelDb: -32.5,
      lastEventMs: 1_700_000_000_000,
      reconfigures: 0,
    });
    const old = parseInstance({ ...instanceRaw(), input_bytes: undefined, input_level_db: null });
    expect(old).toMatchObject({ inputBytes: 0, inputLevelDb: null });
    expect(parseInstance(instanceRaw({ vfo: null, device: null })).vfo).toBeNull();
    expect(() => parseInstance({})).toThrow(ApiError);
  });

  it("builds the start body without empty fields", () => {
    expect(
      startBody({ pluginId: "p", device: "dev0", vfo: 2, freqHz: 433920000.4, config: {} }),
    ).toEqual({
      plugin_id: "p",
      device: "dev0",
      vfo: 2,
      freq_hz: 433920000,
    });
    expect(startBody({ pluginId: "p", config: { baud: [512] } })).toEqual({
      plugin_id: "p",
      config: { baud: [512] },
    });
  });
});

describe("client", () => {
  it("lists plugins", async () => {
    const { api, f } = client(reply(200, { v: 1, ok: true, plugins: [{ manifest: POCSAG_RAW }] }));
    const plugins = await api.listPlugins();
    expect(plugins.map((p) => p.id)).toEqual([POCSAG_ID]);
    expect(sent(f)).toMatchObject({ url: "/api/v1/plugins", method: "GET" });
  });

  it("installs from a path", async () => {
    const { api, f } = client(reply(201, { plugin: { manifest: POCSAG_RAW } }));
    expect((await api.install("/src/p")).id).toBe(POCSAG_ID);
    expect(sent(f)).toMatchObject({
      url: "/api/v1/plugins",
      method: "POST",
      body: { path: "/src/p" },
    });
  });

  it("removes with an encoded id", async () => {
    const { api, f } = client(reply(204));
    await api.remove("org.x/y");
    expect(sent(f)).toMatchObject({ url: "/api/v1/plugins/org.x%2Fy", method: "DELETE" });
  });

  it("starts, stops and restarts instances", async () => {
    const { api, f } = client(reply(200, { instance: instanceRaw() }));
    const started = await api.start({ pluginId: POCSAG_ID, device: "dev0", vfo: 1 });
    expect(started.id).toBe("i-1");
    expect(sent(f).body).toEqual({ plugin_id: POCSAG_ID, device: "dev0", vfo: 1 });
    await api.stop("i-1");
    expect(sent(f, 1)).toMatchObject({ url: "/api/v1/plugin-instances/i-1/stop", method: "POST" });
    await api.restart("i-1");
    expect(sent(f, 2).url).toBe("/api/v1/plugin-instances/i-1/restart");
  });

  it("reconfigures an instance with the complete new config", async () => {
    const { api, f } = client(reply(200, { instance: instanceRaw({ config: { invert: true } }) }));
    const updated = await api.reconfigure("i-1", { invert: true });
    expect(updated.config).toEqual({ invert: true });
    expect(sent(f)).toMatchObject({
      url: "/api/v1/plugin-instances/i-1/reconfigure",
      method: "POST",
      body: { config: { invert: true } },
    });
  });

  it("keeps the field a refused configuration names", async () => {
    const { api } = client(
      reply(400, {
        v: 1,
        ok: false,
        code: "invalid",
        error: "invalid configuration: baud.1: 700 is not one of [512, 1200, 2400]",
        field: "baud.1",
      }),
    );
    await expect(api.reconfigure("i-1", { baud: [512, 700] })).rejects.toMatchObject({
      status: 400,
      code: "invalid",
      field: "baud.1",
      message: expect.stringContaining("700 is not one of") as unknown,
    });
  });

  it("surfaces the server's message and code", async () => {
    const { api } = client(
      reply(409, {
        v: 1,
        ok: false,
        code: "conflict",
        error: "another exclusive instance holds dev0",
      }),
    );
    await expect(api.start({ pluginId: "p" })).rejects.toMatchObject({
      message: "another exclusive instance holds dev0",
      status: 409,
      code: "conflict",
    });
  });

  it("reports 503 when the plugin host is not running", async () => {
    const { api } = client(
      reply(503, { code: "unavailable", error: "plugin host is not running" }),
    );
    await expect(api.listInstances()).rejects.toMatchObject({ status: 503, code: "unavailable" });
  });

  it("rejects a malformed list", async () => {
    const { api } = client(reply(200, { nope: true }));
    await expect(api.listPlugins()).resolves.toEqual([]);
    const bad = client(reply(200, "text"));
    await expect(bad.api.listInstances()).rejects.toThrow(ApiError);
  });
});
