import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { EventApi, EventPage } from "./api";
import type { DecoderEvent } from "./events";
import { DecoderEventStore, eventsUrl, parseEventFrame } from "./store";

class FakeSocket {
  static instances: FakeSocket[] = [];
  onopen: (() => void) | null = null;
  onclose: (() => void) | null = null;
  onmessage: ((ev: { data: unknown }) => void) | null = null;
  onerror: (() => void) | null = null;
  constructor(public url: string) {
    FakeSocket.instances.push(this);
  }
  close() {}
  push(event: unknown) {
    this.onmessage?.({ data: JSON.stringify({ v: 1, type: "decoder_event", event }) });
  }
}

const raw = (id: number, over: Record<string, unknown> = {}) => ({
  id,
  ts_ms: 1_700_000_000_000 + id * 1000,
  device: "rtl0",
  vfo: 1,
  freq_hz: 433_500_000,
  mode: "nfm",
  kind: "dtmf",
  value: `v${id}`,
  meta: {},
  ...over,
});

const parsed = (id: number): DecoderEvent => {
  const e = parseEventFrame(JSON.stringify({ v: 1, type: "decoder_event", event: raw(id) }));
  if (!e) throw new Error("fixture");
  return e;
};

function fakeApi(pages: EventPage[] = [{ events: [parsed(2), parsed(1)], next: null }]) {
  const list = vi.fn<EventApi["list"]>(() =>
    Promise.resolve(pages.shift() ?? { events: [], next: null }),
  );
  const clear = vi.fn<EventApi["clear"]>(() => Promise.resolve(2));
  return { list, clear } satisfies EventApi;
}

const flush = () => new Promise((r) => setTimeout(r, 0));

const make = (api: EventApi) =>
  new DecoderEventStore({
    device: "rtl0",
    api,
    createSocket: (u) => new FakeSocket(u) as unknown as WebSocket,
  });

beforeEach(() => {
  FakeSocket.instances = [];
});
afterEach(() => vi.useRealTimers());

describe("parseEventFrame", () => {
  it("accepts only decoder_event frames with a valid event", () => {
    expect(
      parseEventFrame(JSON.stringify({ v: 1, type: "decoder_event", event: raw(3) }))?.id,
    ).toBe(3);
    expect(parseEventFrame(JSON.stringify({ v: 1, type: "state", event: raw(3) }))).toBeNull();
    expect(
      parseEventFrame(JSON.stringify({ v: 2, type: "decoder_event", event: raw(3) })),
    ).toBeNull();
    expect(parseEventFrame(JSON.stringify({ v: 1, type: "decoder_event", event: {} }))).toBeNull();
    expect(parseEventFrame("nope")).toBeNull();
  });

  it("builds same-origin ws/wss URLs", () => {
    expect(eventsUrl("a b", { protocol: "http:", host: "h" })).toBe(
      "ws://h/api/v1/ws/events?dev=a%20b",
    );
  });
});

describe("DecoderEventStore", () => {
  it("loads history, then appends live events", async () => {
    const api = fakeApi();
    const store = make(api);
    store.start();
    await flush();
    expect(store.getSnapshot().events.map((e) => e.id)).toEqual([2, 1]);
    expect(api.list.mock.calls[0]?.[0]).toBe("rtl0");

    FakeSocket.instances[0]?.onopen?.();
    expect(store.getSnapshot().status).toBe("connected");
    FakeSocket.instances[0]?.push(raw(3));
    FakeSocket.instances[0]?.push(raw(3));
    expect(store.getSnapshot().events.map((e) => e.id)).toEqual([3, 2, 1]);
    store.stop();
  });

  it("pauses and resumes the live feed", async () => {
    const store = make(fakeApi());
    store.start();
    await flush();
    store.pause();
    FakeSocket.instances[0]?.push(raw(3));
    expect(store.getSnapshot().events).toHaveLength(2);
    expect(store.getSnapshot().pending).toHaveLength(1);
    store.resume();
    expect(store.getSnapshot().events).toHaveLength(3);
    store.stop();
  });

  it("a filter change queries the server again", async () => {
    const api = fakeApi([
      { events: [parsed(1)], next: null },
      { events: [], next: null },
    ]);
    const store = make(api);
    store.start();
    await flush();
    store.setFilter({ kinds: ["rds_ps"], q: "abc" });
    await flush();
    expect(api.list).toHaveBeenCalledTimes(2);
    expect(api.list.mock.calls[1]?.[1]).toEqual({ kinds: ["rds_ps"], q: "abc" });
    expect(store.getSnapshot().events).toEqual([]);
    expect(store.getSnapshot().loaded).toBe(true);
    store.stop();
  });

  it("loads older pages with the cursor", async () => {
    const api = fakeApi([
      { events: [parsed(5), parsed(4)], next: "c1" },
      { events: [parsed(3)], next: null },
    ]);
    const store = make(api);
    store.start();
    await flush();
    await store.loadMore();
    expect(api.list.mock.calls[1]?.[2]).toMatchObject({ cursor: "c1" });
    expect(store.getSnapshot().events.map((e) => e.id)).toEqual([5, 4, 3]);
    await store.loadMore();
    expect(api.list).toHaveBeenCalledTimes(2);
    store.stop();
  });

  it("clear deletes on the server and empties the list", async () => {
    const api = fakeApi();
    const store = make(api);
    store.start();
    await flush();
    await expect(store.clear()).resolves.toBe(2);
    expect(api.clear).toHaveBeenCalled();
    expect(store.getSnapshot().events).toEqual([]);
    store.stop();
  });

  it("reports a failed load and retries on refresh", async () => {
    const api = fakeApi();
    api.list.mockRejectedValueOnce(new Error("HTTP 503"));
    const store = make(api);
    store.start();
    await flush();
    expect(store.getSnapshot().error).toBe("HTTP 503");
    store.refresh();
    await flush();
    expect(store.getSnapshot().error).toBeNull();
    expect(store.getSnapshot().events).toHaveLength(2);
    store.stop();
  });

  it("reloads after a reconnect so missed events appear", async () => {
    vi.useFakeTimers();
    const api = fakeApi([
      { events: [parsed(1)], next: null },
      { events: [parsed(2), parsed(1)], next: null },
    ]);
    const store = make(api);
    store.start();
    await vi.advanceTimersByTimeAsync(0);
    FakeSocket.instances[0]?.onopen?.();
    FakeSocket.instances[0]?.onclose?.();
    await vi.advanceTimersByTimeAsync(60_000);
    FakeSocket.instances[1]?.onopen?.();
    await vi.advanceTimersByTimeAsync(0);
    expect(api.list).toHaveBeenCalledTimes(2);
    expect(store.getSnapshot().events.map((e) => e.id)).toEqual([2, 1]);
    store.stop();
  });
});
