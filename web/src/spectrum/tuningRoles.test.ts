import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { toasts } from "../ui";
import { ApiError, type ApiClient, type DeviceState } from "./api";
import { TuningController } from "./tuning";

const state: DeviceState = {
  driver: "rtlsdr",
  centerHz: 100e6,
  sampleRate: 2.048e6,
  gain: { mode: "auto", db: null },
  gainsDb: [0, 10, 20],
  ppm: 0,
  biasTee: false,
  tuningEpoch: 0,
  locks: [],
  window: { startHz: 99e6, endHz: 101e6 },
};

function api(over: Partial<ApiClient> = {}): ApiClient {
  return {
    getState: vi.fn(() => Promise.resolve(state)),
    tune: vi.fn(() => Promise.resolve(state)),
    lock: vi.fn(() => Promise.resolve(state)),
    unlock: vi.fn(() => Promise.resolve(state)),
    ...over,
  };
}

beforeEach(() => {
  vi.useFakeTimers();
  toasts.clear();
});
afterEach(() => {
  vi.useRealTimers();
  toasts.clear();
});

describe("tuning by role", () => {
  it("does not send a tune request for a viewer who may not tune, and says why once", async () => {
    const a = api();
    const t = new TuningController({ device: "dev0", api: a, debounceMs: 5 });
    t.start();
    await vi.advanceTimersByTimeAsync(10);
    t.setAllowed(false);
    t.request({ centerHz: 101e6 });
    t.request({ gain: 10 });
    await vi.advanceTimersByTimeAsync(50);
    expect(a.tune).not.toHaveBeenCalled();
    expect(toasts.getSnapshot()).toHaveLength(1);
    expect(toasts.getSnapshot()[0]).toMatchObject({ title: "Operators and admins only" });
    t.setAllowed(true);
    t.request({ gain: 10 });
    await vi.advanceTimersByTimeAsync(50);
    expect(a.tune).toHaveBeenCalledTimes(1);
  });

  it("a 403 from the server is a toast and no error banner", async () => {
    const a = api({ tune: vi.fn(() => Promise.reject(new ApiError("forbidden", 403))) });
    const t = new TuningController({ device: "dev0", api: a, debounceMs: 5 });
    t.start();
    await vi.advanceTimersByTimeAsync(10);
    t.request({ gain: 10 });
    await vi.advanceTimersByTimeAsync(50);
    expect(t.getSnapshot().error).toBeNull();
    expect(toasts.getSnapshot()[0]).toMatchObject({ tone: "warn", title: "Not allowed" });
  });

  it("an admin can force-release the lock", async () => {
    const a = api();
    const t = new TuningController({ device: "dev0", api: a });
    t.start();
    await t.setLock(false, true);
    expect(a.unlock).toHaveBeenCalledWith("dev0", true);
    await t.setLock(false);
    expect(a.unlock).toHaveBeenLastCalledWith("dev0", false);
  });
});
