import { beforeEach, expect, it, vi } from "vitest";
import { AudioController } from "./controller";
import type { AudioEngine } from "./engine";
import type { AudioFrame } from "./frame";
import { listenKey } from "./listen";

class FakeSocket {
  sent: string[] = [];
  readyState = 1;
  binaryType = "blob";
  onopen: (() => void) | null = null;
  onclose: (() => void) | null = null;
  onmessage: ((ev: { data: unknown }) => void) | null = null;
  onerror: (() => void) | null = null;
  send(data: string) {
    this.sent.push(data);
  }
  close() {
    this.readyState = 3;
  }
}

function setup() {
  const engine = {
    setVfos: vi.fn(),
    setGain: vi.fn(),
    onFrame: vi.fn(),
    setTargetMs: vi.fn(),
    enable: vi.fn(),
    disable: vi.fn().mockResolvedValue(undefined),
    debug: vi.fn(),
  };
  const sockets: FakeSocket[] = [];
  const audio = new AudioController({
    device: "dev0",
    engine: engine as unknown as AudioEngine,
    createSocket: () => {
      const s = new FakeSocket();
      sockets.push(s);
      return s as unknown as WebSocket;
    },
  });
  audio.start();
  sockets[0]?.onopen?.();
  const subscribed = () => {
    const last = sockets[0]?.sent.at(-1);
    return last ? (JSON.parse(last) as { vfos: number[] }).vfos : null;
  };
  const frame = (vfoId: number, powerDbfs = -80): AudioFrame =>
    ({ vfoId, powerDbfs, squelchOpen: true }) as unknown as AudioFrame;
  const feed = (f: AudioFrame) => (audio as unknown as { onFrame(f: AudioFrame): void }).onFrame(f);
  return { audio, engine, subscribed, frame, feed };
}

beforeEach(() => window.localStorage.clear());

it("new VFOs listen by default and are all subscribed", () => {
  const { audio, engine, subscribed } = setup();
  audio.setVfos([1, 2]);
  expect(subscribed()).toEqual([1, 2]);
  expect(engine.setVfos).toHaveBeenLastCalledWith(new Set([1, 2]));
  expect(audio.getSnapshot().levels[1]?.listening).toBe(true);
  expect(audio.getSnapshot().levels[2]?.listening).toBe(true);
});

it("a VFO that is not listened to leaves the mix and the subscription", () => {
  const { audio, engine, subscribed, frame, feed } = setup();
  audio.setVfos([1, 2, 3]);
  audio.setListening(2, false);
  expect(subscribed()).toEqual([1, 3]);
  expect(engine.setVfos).toHaveBeenLastCalledWith(new Set([1, 3]));
  expect(audio.getSnapshot().levels[2]?.listening).toBe(false);
  feed(frame(2));
  expect(engine.onFrame).not.toHaveBeenCalled();
  feed(frame(3));
  expect(engine.onFrame).toHaveBeenCalledTimes(1);
  audio.setListening(2, true);
  expect(subscribed()).toEqual([1, 2, 3]);
});

it("the active VFO stays subscribed for its meter, but is not played", () => {
  const { audio, engine, subscribed, frame, feed } = setup();
  audio.setVfos([1, 2]);
  audio.setListening(1, false);
  audio.setActive(1);
  expect(subscribed()).toEqual([1, 2]);
  expect(engine.setVfos).toHaveBeenLastCalledWith(new Set([2]));
  feed(frame(1, -70));
  expect(engine.onFrame).not.toHaveBeenCalled();
  expect(audio.getMeter(1)?.powerDbfs).toBe(-70);
  audio.setActive(2);
  expect(subscribed()).toEqual([2]);
});

it("several VFOs can play at once", () => {
  const { audio, frame, feed, engine } = setup();
  audio.setVfos([1, 2]);
  feed(frame(1));
  feed(frame(2));
  expect(engine.onFrame).toHaveBeenCalledTimes(2);
});

it("persists the listen set per device and restores it", () => {
  const first = setup();
  first.audio.setVfos([1, 2]);
  first.audio.setListening(2, false);
  expect(window.localStorage.getItem(listenKey("dev0"))).toBe('{"silent":[2]}');
  const second = setup();
  second.audio.setVfos([1, 2, 3]);
  expect(second.audio.getSnapshot().levels[2]?.listening).toBe(false);
  expect(second.audio.getSnapshot().levels[3]?.listening).toBe(true);
  expect(second.subscribed()).toEqual([1, 3]);
});

it("forgets the listen setting of a deleted VFO", () => {
  const { audio } = setup();
  audio.setVfos([1, 2]);
  audio.setListening(2, false);
  audio.setVfos([1]);
  expect(window.localStorage.getItem(listenKey("dev0"))).toBeNull();
  audio.setVfos([1, 2]);
  expect(audio.getSnapshot().levels[2]?.listening).toBe(true);
});

it("keeps about the last seconds of channel levels per VFO", () => {
  const { audio, frame, feed } = setup();
  audio.setVfos([1]);
  const now = vi.spyOn(performance, "now");
  now.mockReturnValue(1000);
  feed(frame(1, -90));
  now.mockReturnValue(5000);
  feed(frame(1, -80));
  now.mockReturnValue(6500);
  feed(frame(1, -70));
  expect(audio.getRecentLevels(1, 3000, 6500)).toEqual([-80, -70]);
  expect(audio.getRecentLevels(2, 3000, 6500)).toEqual([]);
  // Beyond the 5 s retention the oldest sample is gone for good.
  now.mockReturnValue(6200);
  expect(audio.getRecentLevels(1, 10_000, 6200)).toEqual([-80, -70]);
  now.mockRestore();
});
