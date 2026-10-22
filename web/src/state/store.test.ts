import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { notifyUnauthorized } from "../auth/session";
import { parseStateFrame } from "./doc";
import { StateStore, bindState, stateUrl } from "./store";

class FakeSocket {
  static instances: FakeSocket[] = [];
  onopen: (() => void) | null = null;
  onclose: (() => void) | null = null;
  onmessage: ((ev: { data: unknown }) => void) | null = null;
  onerror: (() => void) | null = null;
  closed = false;
  constructor(public url: string) {
    FakeSocket.instances.push(this);
  }
  close() {
    this.closed = true;
  }
  open() {
    this.onopen?.();
  }
  drop() {
    this.onclose?.();
  }
  push(state: unknown) {
    this.onmessage?.({ data: JSON.stringify({ v: 1, type: "state", state }) });
  }
}

const raw = (centerHz: number, extra: Record<string, unknown> = {}) => ({
  v: 1,
  device: "dev0",
  driver: "file",
  center_hz: centerHz,
  sample_rate: 2_000_000,
  gain: null,
  gains_db: [],
  ppm: 0,
  bias_tee: false,
  tuning_epoch: 1,
  vfos: [],
  locks: [],
  window: { start_hz: centerHz - 850_000, end_hz: centerHz + 850_000 },
  ...extra,
});

const make = () =>
  new StateStore({
    device: "dev0",
    url: "ws://x/state",
    createSocket: (u) => new FakeSocket(u) as unknown as WebSocket,
  });

describe("stateUrl", () => {
  it("builds same-origin ws/wss URLs", () => {
    expect(stateUrl("dev 0", { protocol: "https:", host: "h" })).toBe(
      "wss://h/api/v1/ws/state?dev=dev%200",
    );
  });
});

describe("parseStateFrame", () => {
  it("parses a state frame with VFOs, locks and window", () => {
    const doc = parseStateFrame(
      JSON.stringify({
        v: 1,
        type: "state",
        state: raw(100e6, {
          locks: [{ kind: "user", id: "user", since_ms: 5 }],
          vfos: [
            {
              id: 1,
              freq_hz: 101e6,
              mode: "nfm",
              bandwidth_hz: 12500,
              squelch_dbfs: null,
              deemphasis: null,
              offset_hz: 1e6,
              status: "paused",
            },
          ],
        }),
      }),
    );
    expect(doc?.device.locks).toEqual([{ kind: "user", id: "user", sinceMs: 5 }]);
    expect(doc?.device.window).toEqual({ startHz: 100e6 - 850_000, endHz: 100e6 + 850_000 });
    expect(doc?.vfos[0]).toMatchObject({ id: 1, status: "paused" });
  });

  it("falls back to 85 % of the rate when the window is missing", () => {
    const state = raw(100e6);
    delete (state as Record<string, unknown>).window;
    const doc = parseStateFrame(JSON.stringify({ v: 1, type: "state", state }));
    expect(doc?.device.window.endHz).toBeCloseTo(100e6 + 0.425 * 2_000_000);
  });

  it("ignores other frame types and garbage", () => {
    expect(parseStateFrame('{"type":"hello"}')).toBeNull();
    expect(parseStateFrame("nope")).toBeNull();
    expect(parseStateFrame('{"type":"state","state":{"x":1}}')).toBeNull();
  });
});

describe("StateStore", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    FakeSocket.instances = [];
  });
  afterEach(() => vi.useRealTimers());

  it("holds the first document, then replaces it with every frame", () => {
    const store = make();
    const listener = vi.fn();
    store.subscribe(listener);
    store.start();
    const ws = FakeSocket.instances[0]!;
    ws.open();
    expect(store.getSnapshot()).toMatchObject({ doc: null, status: "connected" });
    ws.push(raw(100e6));
    expect(store.getSnapshot().doc?.device.centerHz).toBe(100e6);
    ws.push(raw(101e6, { vfos: [] }));
    expect(store.getSnapshot().doc?.device.centerHz).toBe(101e6);
    expect(listener).toHaveBeenCalled();
    store.stop();
  });

  it("ignores binary and malformed frames", () => {
    const store = make();
    store.start();
    const ws = FakeSocket.instances[0]!;
    ws.open();
    ws.onmessage?.({ data: new ArrayBuffer(4) });
    ws.onmessage?.({ data: "{broken" });
    expect(store.getSnapshot().doc).toBeNull();
    store.stop();
  });

  it("reconnects with backoff and keeps the last document meanwhile", () => {
    const store = make();
    store.start();
    const first = FakeSocket.instances[0]!;
    first.open();
    first.push(raw(100e6));
    first.drop();
    expect(store.getSnapshot()).toMatchObject({ status: "reconnecting" });
    expect(store.getSnapshot().doc?.device.centerHz).toBe(100e6);
    vi.advanceTimersByTime(250);
    expect(FakeSocket.instances).toHaveLength(2);
    FakeSocket.instances[1]!.open();
    FakeSocket.instances[1]!.push(raw(102e6));
    expect(store.getSnapshot()).toMatchObject({ status: "connected" });
    expect(store.getSnapshot().doc?.device.centerHz).toBe(102e6);
    store.stop();
    vi.advanceTimersByTime(60_000);
    expect(FakeSocket.instances).toHaveLength(2);
  });

  it("pauses (no reconnect, no connect) while the session is lost", () => {
    const store = make();
    store.start();
    FakeSocket.instances[0]!.open();
    notifyUnauthorized();
    FakeSocket.instances[0]!.drop();
    vi.advanceTimersByTime(60_000);
    expect(FakeSocket.instances).toHaveLength(1);
    store.stop();

    const again = make();
    again.start();
    expect(FakeSocket.instances).toHaveLength(1);
    again.stop();
  });

  it("asks the server whether the session is valid after a rejected upgrade", async () => {
    vi.stubGlobal(
      "fetch",
      vi
        .fn()
        .mockResolvedValue(new Response(JSON.stringify({ authenticated: false }), { status: 200 })),
    );
    const store = make();
    store.start();
    FakeSocket.instances[0]!.drop(); // never opened
    await vi.advanceTimersByTimeAsync(60_000);
    expect(FakeSocket.instances).toHaveLength(1);
    store.stop();
    vi.unstubAllGlobals();
  });

  it("accepts documents from REST replies via ingest()", () => {
    const store = make();
    const doc = parseStateFrame(JSON.stringify({ type: "state", state: raw(90e6) }))!;
    store.ingest(doc);
    expect(store.getSnapshot().doc).toBe(doc);
  });
});

describe("bindState", () => {
  it("feeds the controllers with the current and every new document", () => {
    const store = make();
    const tuning = { ingest: vi.fn() };
    const vfos = { ingest: vi.fn() };
    const first = parseStateFrame(JSON.stringify({ type: "state", state: raw(90e6) }))!;
    store.ingest(first);
    const off = bindState(store, { tuning, vfos });
    expect(tuning.ingest).toHaveBeenCalledWith(first.device);
    expect(vfos.ingest).toHaveBeenCalledWith(first.vfos);
    const second = parseStateFrame(JSON.stringify({ type: "state", state: raw(91e6) }))!;
    store.ingest(second);
    expect(tuning.ingest).toHaveBeenCalledTimes(2);
    off();
    store.ingest(first);
    expect(tuning.ingest).toHaveBeenCalledTimes(2);
  });
});
