import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { AdsbStore, POLL_MS } from "./store";
import { adsbReply, fakeAdsbApi } from "./testing";

beforeEach(() => vi.useFakeTimers());
afterEach(() => vi.useRealTimers());

describe("AdsbStore", () => {
  it("polls the followed instance every second and stamps the arrival", async () => {
    const { api } = fakeAdsbApi();
    const store = new AdsbStore(api);
    store.start();
    store.follow("i-adsb");
    await vi.advanceTimersByTimeAsync(0);
    expect(api.status).toHaveBeenCalledWith("i-adsb", expect.anything());
    expect(store.getSnapshot().reply?.status?.total).toBe(3);
    expect(store.getSnapshot().receivedAt).toBe(Date.now());
    await vi.advanceTimersByTimeAsync(POLL_MS * 3);
    expect(api.status.mock.calls.length).toBeGreaterThanOrEqual(4);
    store.stop();
  });

  it("does nothing without an instance and forgets the reply when it goes away", async () => {
    const { api } = fakeAdsbApi();
    const store = new AdsbStore(api);
    store.start();
    await vi.advanceTimersByTimeAsync(POLL_MS * 2);
    expect(api.status).not.toHaveBeenCalled();
    store.follow("i-adsb");
    await vi.advanceTimersByTimeAsync(0);
    expect(store.getSnapshot().reply).not.toBeNull();
    store.follow(null);
    expect(store.getSnapshot()).toMatchObject({ instanceId: null, reply: null });
    store.stop();
  });

  it("lets go of an instance that belongs to another plugin", async () => {
    const { api } = fakeAdsbApi(adsbReply({ plugin_id: "org.superhet.decoder.dab" }));
    const store = new AdsbStore(api);
    store.start();
    store.follow("i-dab");
    await vi.advanceTimersByTimeAsync(0);
    expect(store.getSnapshot()).toMatchObject({ instanceId: null, reply: null, error: null });
    await vi.advanceTimersByTimeAsync(POLL_MS * 3);
    expect(api.status).toHaveBeenCalledTimes(1);
    store.stop();
  });

  it("reports a failing status call and recovers", async () => {
    const { api } = fakeAdsbApi();
    api.status.mockRejectedValueOnce(new Error("host down"));
    const store = new AdsbStore(api);
    store.start();
    store.follow("i-adsb");
    await vi.advanceTimersByTimeAsync(0);
    expect(store.getSnapshot().error).toMatch(/host down/);
    await vi.advanceTimersByTimeAsync(POLL_MS);
    expect(store.getSnapshot().error).toBeNull();
    expect(store.getSnapshot().reply).not.toBeNull();
    store.stop();
  });

  it("opens and closes the view and notifies subscribers", () => {
    const store = new AdsbStore(fakeAdsbApi().api);
    const seen = vi.fn();
    store.subscribe(seen);
    store.openView();
    store.openView();
    expect(store.getSnapshot().viewOpen).toBe(true);
    store.closeView();
    expect(store.getSnapshot().viewOpen).toBe(false);
    expect(seen).toHaveBeenCalledTimes(2);
  });
});
