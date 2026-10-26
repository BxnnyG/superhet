import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { GUEST_LIMITS, accountStatus, guestStatus } from "../auth/testing";
import { canModifyVfo, permissionsFor } from "../auth/permissions";
import { createBlockReason } from "../shell/useReceiver";
import { ApiError } from "../spectrum/api";
import { toasts } from "../ui";
import type { Vfo, VfoApi, VfoCreate, VfoReply } from "./api";
import { VfoController } from "./controller";

const vfo = (id: number, over: Partial<Vfo> = {}): Vfo => ({
  id,
  freqHz: 100e6,
  mode: "wfm",
  bandwidthHz: 180_000,
  squelchDbfs: null,
  deemphasis: "50us",
  offsetHz: 0,
  status: "active",
  ...over,
});

function api(list: Vfo[]): VfoApi & {
  update: ReturnType<typeof vi.fn>;
  remove: ReturnType<typeof vi.fn>;
  create: ReturnType<typeof vi.fn>;
} {
  return {
    list: vi.fn(() => Promise.resolve(list)),
    create: vi.fn(),
    update: vi.fn(),
    remove: vi.fn(() => Promise.resolve()),
  } as never;
}

beforeEach(() => {
  vi.useFakeTimers();
  toasts.clear();
});
afterEach(() => {
  vi.useRealTimers();
  toasts.clear();
});

const flush = () => vi.advanceTimersByTimeAsync(100);

async function started(list: Vfo[], a: VfoApi, guards: Parameters<VfoController["setGuards"]>[0]) {
  const c = new VfoController({ device: "dev0", api: a, debounceMs: 10 });
  c.setGuards(guards);
  c.start();
  await flush();
  expect(c.getSnapshot().vfos).toHaveLength(list.length);
  return c;
}

describe("VFO ownership guard", () => {
  const listener = permissionsFor(accountStatus("listener"), null);
  const list = [vfo(1, { mine: true }), vfo(2, { mine: false, shared: false })];

  it("does not send a change or delete of somebody else's receiver and says why", async () => {
    const a = api(list);
    const c = await started(list, a, { canModify: (v) => canModifyVfo(listener, v) });
    c.update(2, { freqHz: 101e6 });
    expect(c.remove(2)).toBe(false);
    await flush();
    expect(a.update).not.toHaveBeenCalled();
    expect(a.remove).not.toHaveBeenCalled();
    expect(c.getSnapshot().vfos[1]?.freqHz).toBe(100e6);
    expect(toasts.getSnapshot()).toHaveLength(1);
    expect(toasts.getSnapshot()[0]).toMatchObject({ title: "Not your receiver" });
  });

  it("sends changes of own receivers", async () => {
    const a = api(list);
    a.update.mockResolvedValue({
      vfo: vfo(1, { freqHz: 101e6, mine: true }),
      vfos: null,
      state: null,
      retune: "none",
    } satisfies VfoReply);
    const c = await started(list, a, { canModify: (v) => canModifyVfo(listener, v) });
    c.update(1, { freqHz: 101e6 });
    await flush();
    expect(a.update).toHaveBeenCalledWith("dev0", 1, expect.objectContaining({ freqHz: 101e6 }));
    expect(toasts.getSnapshot()).toHaveLength(0);
  });

  it("operators may change any receiver", async () => {
    const operator = permissionsFor(accountStatus("operator"), null);
    const a = api(list);
    a.update.mockResolvedValue({ vfo: list[1], vfos: null, state: null, retune: "none" });
    const c = await started(list, a, { canModify: (v) => canModifyVfo(operator, v) });
    c.update(2, { freqHz: 101e6 });
    await flush();
    expect(a.update).toHaveBeenCalled();
  });
});

describe("403 answers become toasts, not error banners", () => {
  const cases: [string, string][] = [
    ["vfo_limit", "Receiver limit reached"],
    ["mode_not_allowed", "Mode not allowed"],
    ["frequency_not_allowed", "Frequency not allowed"],
    ["locked", "Tuning is locked"],
    ["not_owner", "Not your receiver"],
  ];
  it.each(cases)("%s", async (code, title) => {
    const a = api([]);
    a.create.mockRejectedValue(new ApiError("refused", 403, code));
    const c = await started([], a, {});
    c.create({ freqHz: 100e6, mode: "wfm" });
    await flush();
    expect(toasts.getSnapshot()[0]).toMatchObject({ tone: "warn", title });
    expect(c.getSnapshot().error).toBeNull();
  });

  it("other failures stay in the error banner", async () => {
    const a = api([]);
    a.create.mockRejectedValue(new ApiError("device offline", 503));
    const c = await started([], a, {});
    c.create({ freqHz: 100e6, mode: "wfm" });
    await flush();
    expect(c.getSnapshot().error).toBe("device offline");
    expect(toasts.getSnapshot()).toHaveLength(0);
  });
});

describe("guest policy up front", () => {
  const guest = permissionsFor(guestStatus(), GUEST_LIMITS);
  const req: VfoCreate = { freqHz: 100e6, mode: "wfm" };

  it("blocks creating beyond the guest's receiver cap", () => {
    expect(createBlockReason(guest, [{ mine: true }], req)).toMatch(/at most 1 receiver/);
    expect(createBlockReason(guest, [{ mine: false }], req)).toBeNull();
  });

  it("blocks modes and frequencies the policy forbids", () => {
    expect(createBlockReason(guest, [], { freqHz: 7e6, mode: "usb" })).toMatch(/mode/);
    const ranged = permissionsFor(guestStatus(), {
      ...GUEST_LIMITS,
      freqMinHz: 88e6,
      freqMaxHz: 108e6,
    });
    expect(createBlockReason(ranged, [], { freqHz: 145e6, mode: "nfm" })).toMatch(/frequency/);
    expect(createBlockReason(ranged, [], { freqHz: 100e6, mode: "wfm" })).toBeNull();
  });

  it("never blocks accounts: their quota is only known to the server", () => {
    const listener = permissionsFor(accountStatus("listener"), null);
    const many = Array.from({ length: 20 }, () => ({ mine: true }));
    expect(createBlockReason(listener, many, req)).toBeNull();
  });

  it("the controller refuses the request and sends nothing", async () => {
    const a = api([]);
    const c = await started([], a, {
      createBlock: (r, current) => createBlockReason(guest, current, r),
    });
    c.create({ freqHz: 7e6, mode: "usb" });
    await flush();
    expect(a.create).not.toHaveBeenCalled();
    expect(toasts.getSnapshot()[0]).toMatchObject({ title: "Cannot open a receiver" });
  });
});
