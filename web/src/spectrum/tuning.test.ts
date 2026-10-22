import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ApiError, type ApiClient, type DeviceState, type TunePatch } from "./api";
import { TuningController } from "./tuning";

const userLock = { kind: "user", id: "user", sinceMs: 1 };

const base: DeviceState = {
  driver: "rtlsdr",
  centerHz: 100e6,
  sampleRate: 2.048e6,
  gain: { mode: "auto", db: null },
  gainsDb: [0, 10, 20],
  ppm: 0,
  biasTee: false,
  tuningEpoch: 0,
  locks: [],
  window: { startHz: 100e6 - 0.425 * 2.048e6, endHz: 100e6 + 0.425 * 2.048e6 },
};

function fakeApi() {
  const calls: TunePatch[] = [];
  const resolvers: Array<{ resolve: (s: DeviceState) => void; reject: (e: unknown) => void }> = [];
  let state = base;
  const api: ApiClient = {
    getState: vi.fn(() => Promise.resolve(state)),
    lock: vi.fn(() => Promise.resolve({ ...state, locks: [userLock] })),
    unlock: vi.fn(() => Promise.resolve({ ...state, locks: [] })),
    tune: vi.fn((_dev: string, patch: TunePatch) => {
      calls.push(patch);
      return new Promise<DeviceState>((resolve, reject) => resolvers.push({ resolve, reject }));
    }),
  };
  return {
    api,
    calls,
    resolvers,
    setState: (s: DeviceState) => {
      state = s;
    },
  };
}

async function flush() {
  await vi.advanceTimersByTimeAsync(0);
}

describe("TuningController", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it("loads the state on start", async () => {
    const f = fakeApi();
    const t = new TuningController({ device: "dev0", api: f.api });
    t.start();
    await flush();
    expect(t.getSnapshot().state).toEqual(base);
    t.stop();
  });

  it("coalesces a burst into one request, latest value wins", async () => {
    const f = fakeApi();
    const t = new TuningController({ device: "dev0", api: f.api, debounceMs: 50 });
    t.start();
    await flush();
    t.request({ centerHz: 101e6 });
    t.request({ centerHz: 102e6 });
    t.request({ sampleRate: 2.4e6 });
    t.request({ centerHz: 103e6 });
    expect(t.getSnapshot().display?.centerHz).toBe(103e6); // optimistic
    expect(t.getSnapshot().state?.centerHz).toBe(100e6);
    expect(f.calls).toHaveLength(0);
    await vi.advanceTimersByTimeAsync(50);
    expect(f.calls).toEqual([{ centerHz: 103e6, sampleRate: 2.4e6 }]);
    t.stop();
  });

  it("keeps at most one request in flight and sends the rest afterwards", async () => {
    const f = fakeApi();
    const t = new TuningController({ device: "dev0", api: f.api, debounceMs: 50 });
    t.start();
    await flush();
    t.request({ centerHz: 101e6 });
    await vi.advanceTimersByTimeAsync(50);
    expect(f.calls).toHaveLength(1);
    expect(t.getSnapshot().busy).toBe(true);

    t.request({ centerHz: 102e6 });
    t.request({ centerHz: 103e6 });
    await vi.advanceTimersByTimeAsync(500);
    expect(f.calls).toHaveLength(1); // still waiting for the reply
    // Display keeps the sent value merged with the newer pending one.
    expect(t.getSnapshot().display?.centerHz).toBe(103e6);

    f.resolvers[0]?.resolve({ ...base, centerHz: 101e6, tuningEpoch: 1 });
    await flush();
    expect(t.getSnapshot().state?.centerHz).toBe(101e6);
    expect(t.getSnapshot().display?.centerHz).toBe(103e6);
    await vi.advanceTimersByTimeAsync(50);
    expect(f.calls).toEqual([{ centerHz: 101e6 }, { centerHz: 103e6 }]);

    f.resolvers[1]?.resolve({ ...base, centerHz: 103e6, tuningEpoch: 2 });
    await flush();
    expect(t.getSnapshot()).toMatchObject({ busy: false, error: null });
    expect(t.getSnapshot().display?.centerHz).toBe(103e6);
    t.stop();
  });

  it("reports errors, drops pending changes and reverts to the device state", async () => {
    const f = fakeApi();
    const t = new TuningController({ device: "dev0", api: f.api, debounceMs: 50 });
    t.start();
    await flush();
    t.request({ gain: 10 });
    await vi.advanceTimersByTimeAsync(50);
    f.resolvers[0]?.reject(new ApiError("gain unsupported", 400, "unsupported"));
    await flush();
    const snap = t.getSnapshot();
    expect(snap.error).toBe("gain unsupported");
    expect(snap.busy).toBe(false);
    expect(snap.display?.gain).toEqual(base.gain);
    t.stop();
  });

  it("does not poll: one read at start, later state comes from ingest()", async () => {
    const f = fakeApi();
    const t = new TuningController({ device: "dev0", api: f.api, debounceMs: 50 });
    t.start();
    await flush();
    await vi.advanceTimersByTimeAsync(30_000);
    expect(f.api.getState).toHaveBeenCalledTimes(1);
    const pushed = { ...base, centerHz: 90e6 };
    t.ingest(pushed);
    expect(t.getSnapshot().state?.centerHz).toBe(90e6);
    expect(t.getSnapshot().display?.centerHz).toBe(90e6);
    t.stop();
  });

  it("ignores pushed state while a change is in flight", async () => {
    const f = fakeApi();
    const t = new TuningController({ device: "dev0", api: f.api, debounceMs: 50 });
    t.start();
    await flush();
    t.request({ centerHz: 95e6 });
    await vi.advanceTimersByTimeAsync(50);
    t.ingest({ ...base, centerHz: 80e6 });
    expect(t.getSnapshot().state?.centerHz).toBe(100e6);
    f.resolvers[0]?.resolve({ ...base, centerHz: 95e6 });
    await flush();
    expect(t.getSnapshot().state?.centerHz).toBe(95e6);
    t.stop();
  });

  it("stops cleanly on stop()", async () => {
    const f = fakeApi();
    const t = new TuningController({ device: "dev0", api: f.api });
    t.start();
    await flush();
    t.stop();
    await vi.advanceTimersByTimeAsync(10_000);
    expect(f.api.getState).toHaveBeenCalledTimes(1);
  });

  describe("tuning lock", () => {
    const locked = { ...base, locks: [userLock] };

    it("refuses centre and rate changes while locked and reports the holders", async () => {
      const f = fakeApi();
      const onLocked = vi.fn();
      const t = new TuningController({ device: "dev0", api: f.api, debounceMs: 50, onLocked });
      t.start();
      await flush();
      t.ingest(locked);
      t.request({ centerHz: 101e6 });
      await vi.advanceTimersByTimeAsync(100);
      expect(f.calls).toHaveLength(0);
      expect(onLocked).toHaveBeenCalledWith([userLock], expect.any(String));
      expect(t.getSnapshot().display?.centerHz).toBe(100e6);
      t.stop();
    });

    it("still sends gain changes and repeats of the current centre", async () => {
      const f = fakeApi();
      const onLocked = vi.fn();
      const t = new TuningController({ device: "dev0", api: f.api, debounceMs: 50, onLocked });
      t.start();
      await flush();
      t.ingest(locked);
      t.request({ centerHz: 100e6, gain: 10 });
      await vi.advanceTimersByTimeAsync(50);
      expect(f.calls).toEqual([{ centerHz: 100e6, gain: 10 }]);
      t.request({ centerHz: 101e6, ppm: 2 });
      expect(onLocked).toHaveBeenCalledTimes(1);
      t.stop();
    });

    it("turns a 409 locked reply into a notice instead of an error banner", async () => {
      const f = fakeApi();
      const onLocked = vi.fn();
      const t = new TuningController({ device: "dev0", api: f.api, debounceMs: 50, onLocked });
      t.start();
      await flush();
      t.request({ centerHz: 101e6 });
      await vi.advanceTimersByTimeAsync(50);
      f.resolvers[0]?.reject(new ApiError("tuning is locked by: user", 409, "locked", [userLock]));
      await flush();
      expect(onLocked).toHaveBeenCalledWith([userLock], "tuning is locked by: user");
      expect(t.getSnapshot().error).toBeNull();
      expect(t.getSnapshot().display?.centerHz).toBe(100e6);
      t.stop();
    });

    it("takes and drops the user lock", async () => {
      const f = fakeApi();
      const t = new TuningController({ device: "dev0", api: f.api });
      t.start();
      await flush();
      await t.setLock(true);
      expect(t.getSnapshot().state?.locks).toEqual([userLock]);
      expect(t.getSnapshot().lockBusy).toBe(false);
      await t.setLock(false);
      expect(t.getSnapshot().state?.locks).toEqual([]);
      t.stop();
    });
  });
});
