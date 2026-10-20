import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { SpectrumSocket, backoffDelay, clampBins, spectrumUrl } from "./socket";

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

describe("helpers", () => {
  it("builds same-origin ws/wss URLs", () => {
    expect(spectrumUrl("dev0", { protocol: "http:", host: "h:1" })).toBe(
      "ws://h:1/api/v1/ws/spectrum?dev=dev0",
    );
    expect(spectrumUrl("dev0", { protocol: "https:", host: "h" })).toBe(
      "wss://h/api/v1/ws/spectrum?dev=dev0",
    );
  });

  it("backs off from 250 ms up to 10 s", () => {
    expect([0, 1, 2, 10].map(backoffDelay)).toEqual([250, 500, 1000, 10_000]);
  });

  it("clamps bins to 16..4096", () => {
    expect([1, 800.4, 99999, NaN].map(clampBins)).toEqual([16, 800, 4096, 16]);
  });
});

describe("SpectrumSocket", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.stubGlobal("WebSocket", Object.assign(FakeSocket, { OPEN: 1 }));
    FakeSocket.instances = [];
  });
  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  it("sends a view on open and reconnects with backoff", () => {
    const statuses: string[] = [];
    const sock = new SpectrumSocket({
      device: "dev0",
      getBins: () => 1000,
      onFrame: () => undefined,
      onStatus: (s) => statuses.push(s),
      url: "ws://x/y",
      createSocket: (u) => new FakeSocket(u) as unknown as WebSocket,
    });
    sock.start();
    const first = FakeSocket.instances[0]!;
    first.open();
    expect(JSON.parse(first.sent[0]!)).toMatchObject({ type: "view", v: 1, bins: 1000 });
    first.drop();
    expect(FakeSocket.instances).toHaveLength(1);
    vi.advanceTimersByTime(250);
    expect(FakeSocket.instances).toHaveLength(2);
    expect(statuses).toEqual(["connecting", "connected", "reconnecting"]);
    sock.stop();
    vi.advanceTimersByTime(20_000);
    expect(FakeSocket.instances).toHaveLength(2);
  });
});
