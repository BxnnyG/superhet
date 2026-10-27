import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ApiError } from "../spectrum/api";
import { DabStore, POLL_MS } from "./store";
import { dabReply, fakeDabApi, fakeEventApi } from "./testing";

beforeEach(() => vi.useFakeTimers());
afterEach(() => vi.useRealTimers());

const flush = () => vi.advanceTimersByTimeAsync(0);

describe("DabStore", () => {
  it("does nothing without an instance, polls the status once one is followed", async () => {
    const { api } = fakeDabApi();
    const store = new DabStore("dev0", api, fakeEventApi());
    store.start();
    await vi.advanceTimersByTimeAsync(POLL_MS * 2);
    expect(api.status).not.toHaveBeenCalled();

    store.follow("i-1");
    await flush();
    expect(api.status).toHaveBeenCalledWith("i-1", expect.anything());
    expect(store.getSnapshot().reply?.status?.ensemble?.label).toBe("DR Deutschland");
    await vi.advanceTimersByTimeAsync(POLL_MS);
    expect(api.status.mock.calls.length).toBeGreaterThanOrEqual(2);

    store.follow(null);
    const calls = api.status.mock.calls.length;
    await vi.advanceTimersByTimeAsync(POLL_MS * 3);
    expect(api.status.mock.calls.length).toBe(calls);
    expect(store.getSnapshot().reply).toBeNull();
    store.stop();
  });

  it("ignores the claim of another plugin", async () => {
    const { api } = fakeDabApi(dabReply({ plugin_id: "org.example.decoder.adsb" }));
    const store = new DabStore("dev0", api, fakeEventApi());
    store.start();
    store.follow("i-9");
    await flush();
    expect(store.getSnapshot()).toMatchObject({ instanceId: null, reply: null });
    store.stop();
  });

  it("shows a failing status and recovers", async () => {
    const { api } = fakeDabApi();
    api.status.mockRejectedValueOnce(new ApiError("the plugin host is not running", 503));
    const store = new DabStore("dev0", api, fakeEventApi());
    store.start();
    store.follow("i-1");
    await flush();
    expect(store.getSnapshot().error).toBe("the plugin host is not running");
    await vi.advanceTimersByTimeAsync(POLL_MS);
    expect(store.getSnapshot().error).toBeNull();
    expect(store.getSnapshot().reply).not.toBeNull();
    store.stop();
  });

  it("sends control messages to the followed instance", async () => {
    const { api } = fakeDabApi();
    const store = new DabStore("dev0", api, fakeEventApi());
    store.start();
    await expect(store.control({ cmd: "scan_stop" })).rejects.toThrow(/not running/);
    store.follow("i-1");
    await flush();
    await store.control({ cmd: "select", service: "D212" });
    expect(api.control).toHaveBeenCalledWith("i-1", { cmd: "select", service: "D212" });
    store.stop();
  });

  it("remembers a picked block until it is confirmed or dropped", () => {
    const store = new DabStore("dev0", fakeDabApi().api, fakeEventApi());
    store.request({ channel: "5C", freqHz: 178_352_000 });
    expect(store.getSnapshot().pending).toEqual({ channel: "5C", freqHz: 178_352_000 });
    store.request(null);
    expect(store.getSnapshot().pending).toBeNull();
  });

  it("loads the ensembles found earlier from the decoder history", async () => {
    const events = fakeEventApi([
      {
        id: 1,
        tsMs: 1,
        device: "dev0",
        vfo: null,
        freqHz: 178352000,
        mode: null,
        kind: "dab.ensemble",
        source: "org.superhet.decoder.dab",
        value: "5C: DR Deutschland (8 services)",
        meta: {
          data: {
            channel: "5C",
            freq_hz: 178352000,
            label: "DR Deutschland",
            eid: "D210",
            services: [1, 2],
          },
        },
      },
    ]);
    const store = new DabStore("dev0", fakeDabApi().api, events);
    store.start();
    await flush();
    expect(events.list).toHaveBeenCalledWith(
      "dev0",
      { kinds: ["dab.ensemble"], source: "org.superhet.decoder.dab", q: "" },
      expect.objectContaining({ limit: 400 }),
    );
    expect(store.getSnapshot().known).toMatchObject([
      { channel: "5C", label: "DR Deutschland", services: 2 },
    ]);
    store.stop();
  });
});
