import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ApiError } from "../spectrum/api";
import { POLL_MS, PluginStore, instancesOnDevice, instancesOnVfo, pluginName } from "./store";
import { POCSAG_ID, fakePluginApi, instance, pocsag, rtl433 } from "./testing";

beforeEach(() => vi.useFakeTimers());
afterEach(() => vi.useRealTimers());

const flush = () => vi.advanceTimersByTimeAsync(0);

describe("PluginStore", () => {
  it("loads plugins and instances at start and polls the instances", async () => {
    const { api, state } = fakePluginApi();
    const store = new PluginStore(api);
    store.start();
    await flush();
    expect(store.getSnapshot()).toMatchObject({ loaded: true, error: null });
    expect(store.getSnapshot().plugins).toHaveLength(2);
    expect(store.getSnapshot().instances).toEqual([]);

    state.instances = [instance()];
    await vi.advanceTimersByTimeAsync(POLL_MS);
    expect(store.getSnapshot().instances).toHaveLength(1);
    // Polling asks for instances only; the plugin list is not re-read.
    expect(api.listPlugins).toHaveBeenCalledTimes(1);
    store.stop();
    await vi.advanceTimersByTimeAsync(POLL_MS * 3);
    expect(api.listInstances).toHaveBeenCalledTimes(2);
  });

  it("shows why the lists are unavailable and recovers", async () => {
    const { api } = fakePluginApi();
    api.listPlugins.mockRejectedValueOnce(
      new ApiError("plugin host is not running", 503, "unavailable"),
    );
    const store = new PluginStore(api);
    store.start();
    await flush();
    expect(store.getSnapshot()).toMatchObject({
      loaded: true,
      error: "plugin host is not running",
    });
    await vi.advanceTimersByTimeAsync(POLL_MS);
    expect(store.getSnapshot()).toMatchObject({ error: null });
    expect(store.getSnapshot().plugins).toHaveLength(2);
    store.stop();
  });

  it("starts, stops and restarts through the API and refreshes at once", async () => {
    const { api } = fakePluginApi();
    const store = new PluginStore(api);
    store.start();
    await flush();
    await store.startInstance({ pluginId: POCSAG_ID, device: "dev0", vfo: 1 });
    expect(store.getSnapshot().instances).toHaveLength(1);
    await store.restartInstance("i-1");
    expect(api.restart).toHaveBeenCalledWith("i-1");
    await store.stopInstance("i-1");
    expect(store.getSnapshot().instances).toHaveLength(0);
    store.stop();
  });

  it("passes server errors of a start to the caller", async () => {
    const { api } = fakePluginApi();
    api.start.mockRejectedValueOnce(new ApiError("device is locked", 409, "conflict"));
    const store = new PluginStore(api);
    await expect(store.startInstance({ pluginId: "p" })).rejects.toThrow("device is locked");
  });

  it("notifies subscribers", async () => {
    const { api } = fakePluginApi();
    const store = new PluginStore(api);
    const seen = vi.fn();
    const off = store.subscribe(seen);
    store.start();
    await flush();
    expect(seen).toHaveBeenCalled();
    off();
    store.stop();
  });
});

describe("selectors", () => {
  const list = [
    instance({ id: "a", vfo: 1 }),
    instance({ id: "b", vfo: 2 }),
    instance({ id: "c", vfo: null, exclusive: true }),
    instance({ id: "d", device: "dev1", vfo: 1 }),
  ];

  it("finds the instances of a VFO and of the device", () => {
    expect(instancesOnVfo(list, "dev0", 1).map((i) => i.id)).toEqual(["a"]);
    expect(instancesOnDevice(list, "dev0").map((i) => i.id)).toEqual(["c"]);
  });

  it("names plugins", () => {
    expect(pluginName([pocsag, rtl433], POCSAG_ID)).toBe("POCSAG / FLEX pagers");
    expect(pluginName([], "org.example.decoder.thing")).toBe("thing");
  });
});
