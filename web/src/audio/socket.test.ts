import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { AudioSocket, audioUrl, normalizeVfoIds, subscribeMessage } from "./socket";

class FakeSocket {
  static instances: FakeSocket[] = [];
  readyState = 0;
  binaryType = "blob";
  sent: string[] = [];
  onopen: (() => void) | null = null;
  onclose: (() => void) | null = null;
  onmessage: ((ev: { data: unknown }) => void) | null = null;
  onerror: (() => void) | null = null;
  constructor(public url: string) {
    FakeSocket.instances.push(this);
  }
  send(data: string) {
    this.sent.push(data);
  }
  close() {
    this.readyState = 3;
  }
  open() {
    this.readyState = 1;
    this.onopen?.();
  }
  drop() {
    this.readyState = 3;
    this.onclose?.();
  }
}

describe("subscription helpers", () => {
  it("builds same-origin ws/wss URLs", () => {
    expect(audioUrl("dev0", { protocol: "http:", host: "h:1" })).toBe(
      "ws://h:1/api/v1/ws/audio?dev=dev0",
    );
    expect(audioUrl("a b", { protocol: "https:", host: "h" })).toBe(
      "wss://h/api/v1/ws/audio?dev=a%20b",
    );
  });

  it("normalises the id list: unique, >= 1, integers, sorted, at most 8", () => {
    expect(normalizeVfoIds([3, 1, 3, 0, -2, 1.5, NaN, 2])).toEqual([1, 2, 3]);
    expect(normalizeVfoIds([9, 8, 7, 6, 5, 4, 3, 2, 1, 10])).toEqual([1, 2, 3, 4, 5, 6, 7, 8]);
  });

  it("serialises the subscribe message", () => {
    expect(JSON.parse(subscribeMessage([2, 1]))).toEqual({ type: "subscribe", v: 1, vfos: [1, 2] });
    expect(JSON.parse(subscribeMessage([]))).toEqual({ type: "subscribe", v: 1, vfos: [] });
  });
});

describe("AudioSocket", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.stubGlobal("WebSocket", Object.assign(FakeSocket, { OPEN: 1 }));
    FakeSocket.instances = [];
  });
  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  const make = (extra: Partial<ConstructorParameters<typeof AudioSocket>[0]> = {}) =>
    new AudioSocket({
      device: "dev0",
      onFrame: () => undefined,
      createSocket: (url) => new FakeSocket(url) as unknown as WebSocket,
      ...extra,
    });

  it("subscribes on open with the pending set", () => {
    const s = make();
    s.setVfos([2, 1]);
    s.start();
    const ws = FakeSocket.instances[0];
    expect(ws?.url).toContain("/api/v1/ws/audio?dev=dev0");
    ws?.open();
    expect(ws?.sent.map((m) => JSON.parse(m))).toEqual([{ type: "subscribe", v: 1, vfos: [1, 2] }]);
    s.stop();
  });

  it("sends changes immediately and skips repeats", () => {
    const s = make();
    s.start();
    const ws = FakeSocket.instances[0];
    ws?.open(); // sends the empty set
    s.setVfos([1]);
    s.setVfos([1, 1]);
    s.setVfos([1, 3]);
    expect(ws?.sent.map((m) => JSON.parse(m).vfos)).toEqual([[], [1], [1, 3]]);
    s.stop();
  });

  it("resubscribes after a reconnect with backoff", () => {
    const statuses: string[] = [];
    const s = make({ onStatus: (st) => statuses.push(st) });
    s.start();
    s.setVfos([5]);
    FakeSocket.instances[0]?.open();
    FakeSocket.instances[0]?.drop();
    vi.advanceTimersByTime(249);
    expect(FakeSocket.instances).toHaveLength(1);
    vi.advanceTimersByTime(1);
    expect(FakeSocket.instances).toHaveLength(2);
    FakeSocket.instances[1]?.open();
    expect(JSON.parse(FakeSocket.instances[1]?.sent[0] ?? "{}").vfos).toEqual([5]);
    expect(statuses).toEqual(["connecting", "connected", "reconnecting", "connected"]);
    s.stop();
  });

  it("delivers decoded frames and reports bad ones and server errors", () => {
    const frames: number[] = [];
    const errors: string[] = [];
    const s = make({ onFrame: (f) => frames.push(f.vfoId), onError: (m) => errors.push(m) });
    s.start();
    const ws = FakeSocket.instances[0];
    ws?.open();
    const buf = new ArrayBuffer(42);
    const v = new DataView(buf);
    "SHAU".split("").forEach((c, i) => v.setUint8(i, c.charCodeAt(0)));
    v.setUint8(4, 1);
    v.setUint8(5, 1);
    v.setUint8(6, 1);
    v.setUint32(12, 4, true);
    ws?.onmessage?.({ data: buf });
    ws?.onmessage?.({ data: new ArrayBuffer(10) });
    ws?.onmessage?.({ data: JSON.stringify({ v: 1, type: "error", error: "too many" }) });
    ws?.onmessage?.({ data: JSON.stringify({ v: 1, type: "ack", vfos: [] }) });
    expect(frames).toEqual([4]);
    expect(errors).toHaveLength(2);
    expect(errors[1]).toBe("too many");
    s.stop();
  });
});
