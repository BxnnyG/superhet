import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ApiError } from "../spectrum/api";
import type { Vfo, VfoApi, VfoCreate, VfoPatch, VfoReply } from "./api";
import { VfoController, applyVfoPatch } from "./controller";

const vfo = (id: number, freqHz = 100e6): Vfo => ({
  id,
  freqHz,
  mode: "wfm",
  bandwidthHz: 180_000,
  squelchDbfs: null,
  deemphasis: "50us",
  offsetHz: 0,
  status: "active",
});

type Pending<T> = { resolve: (v: T) => void; reject: (e: unknown) => void };

function fakeApi(initial: Vfo[] = []) {
  let list = initial;
  const updates: Array<{ id: number; patch: VfoPatch }> = [];
  const creates: VfoCreate[] = [];
  const removes: number[] = [];
  const updateWaiters: Array<Pending<VfoReply>> = [];
  const createWaiters: Array<Pending<VfoReply>> = [];
  const removeWaiters: Array<Pending<void>> = [];
  const api: VfoApi = {
    list: vi.fn(() => Promise.resolve(list)),
    create: vi.fn((_d: string, req: VfoCreate) => {
      creates.push(req);
      return new Promise<VfoReply>((resolve, reject) => createWaiters.push({ resolve, reject }));
    }),
    update: vi.fn((_d: string, id: number, patch: VfoPatch) => {
      updates.push({ id, patch });
      return new Promise<VfoReply>((resolve, reject) => updateWaiters.push({ resolve, reject }));
    }),
    remove: vi.fn((_d: string, id: number) => {
      removes.push(id);
      return new Promise<void>((resolve, reject) => removeWaiters.push({ resolve, reject }));
    }),
  };
  return {
    api,
    updates,
    creates,
    removes,
    updateWaiters,
    createWaiters,
    removeWaiters,
    setList: (l: Vfo[]) => {
      list = l;
    },
  };
}

const reply = (v: Vfo, vfos: Vfo[] | null, retune: VfoReply["retune"] = "none"): VfoReply => ({
  vfo: v,
  vfos,
  state: null,
  retune,
});

const flush = () => vi.advanceTimersByTimeAsync(0);

describe("applyVfoPatch", () => {
  it("applies set fields and ignores a bandwidth reset", () => {
    const v = applyVfoPatch(vfo(1), { freqHz: 5, bandwidthHz: null, squelchDbfs: -50 });
    expect(v).toMatchObject({ freqHz: 5, bandwidthHz: 180_000, squelchDbfs: -50 });
    expect(applyVfoPatch(v, { squelchDbfs: null }).squelchDbfs).toBeNull();
  });
});

describe("VfoController", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  const make = (f: ReturnType<typeof fakeApi>) =>
    new VfoController({ device: "dev0", api: f.api, debounceMs: 50 });

  it("loads the list on start", async () => {
    const f = fakeApi([vfo(1), vfo(2)]);
    const c = make(f);
    c.start();
    await flush();
    expect(c.getSnapshot().vfos.map((v) => v.id)).toEqual([1, 2]);
    expect(c.getSnapshot().loaded).toBe(true);
    c.stop();
  });

  it("coalesces a burst of updates per VFO, latest value per field wins", async () => {
    const f = fakeApi([vfo(1)]);
    const c = make(f);
    c.start();
    await flush();
    c.update(1, { freqHz: 101e6 });
    c.update(1, { freqHz: 102e6 });
    c.update(1, { squelchDbfs: -60 });
    c.update(1, { freqHz: 103e6 });
    // Optimistic display before anything was sent.
    expect(c.getSnapshot().vfos[0]).toMatchObject({ freqHz: 103e6, squelchDbfs: -60 });
    expect(c.getSnapshot().busy).toBe(true);
    await vi.advanceTimersByTimeAsync(49);
    expect(f.updates).toHaveLength(0);
    await vi.advanceTimersByTimeAsync(1);
    expect(f.updates).toEqual([{ id: 1, patch: { freqHz: 103e6, squelchDbfs: -60 } }]);
    c.stop();
  });

  it("keeps one request in flight and sends the newest values afterwards", async () => {
    const f = fakeApi([vfo(1)]);
    const c = make(f);
    c.start();
    await flush();
    c.update(1, { freqHz: 101e6 });
    await vi.advanceTimersByTimeAsync(50);
    expect(f.updates).toHaveLength(1);
    // While in flight: more changes, still only one request.
    c.update(1, { freqHz: 102e6 });
    c.update(1, { freqHz: 103e6 });
    await vi.advanceTimersByTimeAsync(500);
    expect(f.updates).toHaveLength(1);
    expect(c.getSnapshot().vfos[0]?.freqHz).toBe(103e6);
    f.updateWaiters[0]?.resolve(reply(vfo(1, 101e6), [vfo(1, 101e6)]));
    await vi.advanceTimersByTimeAsync(50);
    expect(f.updates).toHaveLength(2);
    expect(f.updates[1]).toEqual({ id: 1, patch: { freqHz: 103e6 } });
    f.updateWaiters[1]?.resolve(reply(vfo(1, 103e6), [vfo(1, 103e6)]));
    await flush();
    expect(c.getSnapshot()).toMatchObject({ busy: false, error: null });
    expect(c.getSnapshot().vfos[0]?.freqHz).toBe(103e6);
    c.stop();
  });

  it("sends updates of different VFOs one after another", async () => {
    const f = fakeApi([vfo(1), vfo(2)]);
    const c = make(f);
    c.start();
    await flush();
    c.update(1, { freqHz: 101e6 });
    c.update(2, { freqHz: 102e6 });
    await vi.advanceTimersByTimeAsync(50);
    expect(f.updates.map((u) => u.id)).toEqual([1]);
    f.updateWaiters[0]?.resolve(reply(vfo(1, 101e6), null));
    await vi.advanceTimersByTimeAsync(50);
    expect(f.updates.map((u) => u.id)).toEqual([1, 2]);
    c.stop();
  });

  it("selects a created VFO and takes the list from the reply", async () => {
    const f = fakeApi([]);
    const created: number[] = [];
    const c = new VfoController({
      device: "dev0",
      api: f.api,
      debounceMs: 50,
      onCreated: (v) => created.push(v.id),
    });
    c.start();
    await flush();
    c.create({ freqHz: 100e6, mode: "wfm" });
    await vi.advanceTimersByTimeAsync(50);
    expect(f.creates).toEqual([{ freqHz: 100e6, mode: "wfm" }]);
    f.createWaiters[0]?.resolve(reply(vfo(7), [vfo(7)]));
    await flush();
    expect(c.getSnapshot()).toMatchObject({ selectedId: 7, busy: false });
    expect(c.getSnapshot().vfos.map((v) => v.id)).toEqual([7]);
    expect(created).toEqual([7]);
    c.stop();
  });

  it("hides a VFO immediately on delete and deselects it", async () => {
    const f = fakeApi([vfo(1), vfo(2)]);
    const c = make(f);
    c.start();
    await flush();
    c.select(2);
    c.update(2, { freqHz: 5 }); // pending update must be discarded
    c.remove(2);
    expect(c.getSnapshot().vfos.map((v) => v.id)).toEqual([1]);
    expect(c.getSnapshot().selectedId).toBeNull();
    await vi.advanceTimersByTimeAsync(50);
    expect(f.removes).toEqual([2]);
    expect(f.updates).toHaveLength(0);
    f.removeWaiters[0]?.resolve();
    await flush();
    expect(c.getSnapshot().vfos.map((v) => v.id)).toEqual([1]);
    c.stop();
  });

  it("reports an error, drops that VFO's pending change and re-reads the list", async () => {
    const f = fakeApi([vfo(1)]);
    const c = make(f);
    c.start();
    await flush();
    c.update(1, { freqHz: 101e6 });
    await vi.advanceTimersByTimeAsync(50);
    c.update(1, { freqHz: 102e6 });
    f.updateWaiters[0]?.reject(new ApiError("bad bandwidth", 400, "invalid"));
    await flush();
    expect(c.getSnapshot().error).toBe("bad bandwidth");
    expect(f.updates).toHaveLength(1);
    expect(c.getSnapshot().vfos[0]?.freqHz).toBe(100e6);
    expect(c.getSnapshot().busy).toBe(false);
    c.stop();
  });

  it("does not poll; pushed lists replace the confirmed list when idle", async () => {
    const f = fakeApi([vfo(1)]);
    const c = make(f);
    c.start();
    await flush();
    await vi.advanceTimersByTimeAsync(30_000);
    expect(f.api.list).toHaveBeenCalledTimes(1);
    c.ingest([vfo(1, 90e6), vfo(2)]);
    expect(c.getSnapshot().vfos.map((v) => v.freqHz)).toEqual([90e6, 100e6]);
    c.stop();
  });

  it("does not let pushed lists overwrite pending or in-flight changes", async () => {
    const f = fakeApi([vfo(1)]);
    const c = make(f);
    c.start();
    await flush();
    c.update(1, { freqHz: 101e6 });
    c.ingest([vfo(1, 90e6)]); // pending
    expect(c.getSnapshot().vfos[0]?.freqHz).toBe(101e6);
    await vi.advanceTimersByTimeAsync(50);
    c.ingest([vfo(1, 90e6)]); // in flight
    expect(c.getSnapshot().vfos[0]?.freqHz).toBe(101e6);
    f.updateWaiters[0]?.resolve(reply(vfo(1, 101e6), [vfo(1, 101e6)]));
    await flush();
    c.ingest([vfo(1, 90e6)]);
    expect(c.getSnapshot().vfos[0]?.freqHz).toBe(90e6);
    c.stop();
  });

  it("drops the selection when a pushed list no longer has the VFO", async () => {
    const f = fakeApi([vfo(1), vfo(2)]);
    const c = make(f);
    c.start();
    await flush();
    c.select(2);
    c.ingest([vfo(1)]);
    expect(c.getSnapshot().selectedId).toBeNull();
    c.stop();
  });

  it("reports the free-tuning outcome and the reply state", async () => {
    const f = fakeApi([vfo(1)]);
    const onRetune = vi.fn();
    const onState = vi.fn();
    const c = new VfoController({ device: "dev0", api: f.api, debounceMs: 50, onRetune, onState });
    c.start();
    await flush();
    const paused = { ...vfo(1, 120e6), status: "paused" as const };
    c.update(1, { freqHz: 120e6 });
    await vi.advanceTimersByTimeAsync(50);
    const state = { driver: "file" } as unknown as NonNullable<VfoReply["state"]>;
    f.updateWaiters[0]?.resolve({ ...reply(paused, [paused], "locked"), state });
    await flush();
    expect(onRetune).toHaveBeenCalledWith("locked", paused);
    expect(onState).toHaveBeenCalledWith(state);
    expect(c.getSnapshot().vfos[0]?.status).toBe("paused");
    c.update(1, { freqHz: 100e6 });
    await vi.advanceTimersByTimeAsync(50);
    f.updateWaiters[1]?.resolve(reply(vfo(1), [vfo(1)]));
    await flush();
    expect(onRetune).toHaveBeenCalledTimes(1);
    c.stop();
  });

  it("notifies subscribers and stops cleanly", async () => {
    const f = fakeApi([vfo(1)]);
    const c = make(f);
    const listener = vi.fn();
    const off = c.subscribe(listener);
    c.start();
    await flush();
    expect(listener).toHaveBeenCalled();
    off();
    listener.mockClear();
    c.update(1, { freqHz: 5 });
    c.stop();
    await vi.advanceTimersByTimeAsync(5000);
    expect(f.updates).toHaveLength(0);
    expect(listener).not.toHaveBeenCalled();
  });
});
