import { fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createBody, parseVfo, patchBody } from "./api";
import type { Vfo, VfoApi, VfoCreate, VfoPatch, VfoReply } from "./api";
import { VfoController } from "./controller";
import { CwPitchControl } from "./controls";
import {
  CW_PITCH_MAX_HZ,
  CW_PITCH_MIN_HZ,
  clampCwPitch,
  loadCwPitch,
  saveCwPitch,
} from "./cwPitch";

const cw = (over: Partial<Vfo> = {}): Vfo => ({
  id: 1,
  freqHz: 7_030_000,
  mode: "cw",
  bandwidthHz: 500,
  squelchDbfs: null,
  deemphasis: null,
  cwPitchHz: 700,
  offsetHz: 0,
  status: "active",
  ...over,
});

beforeEach(() => localStorage.clear());

describe("remembered pitch", () => {
  it("clamps to the slider range and snaps to its step", () => {
    expect(clampCwPitch(100)).toBe(CW_PITCH_MIN_HZ);
    expect(clampCwPitch(5000)).toBe(CW_PITCH_MAX_HZ);
    expect(clampCwPitch(653)).toBe(650);
    expect(clampCwPitch(Number.NaN)).toBe(700);
  });

  it("stores a non-default pitch and ignores junk and the default", () => {
    expect(loadCwPitch()).toBeNull();
    saveCwPitch(600);
    expect(loadCwPitch()).toBe(600);
    saveCwPitch(700);
    expect(loadCwPitch()).toBeNull();
    localStorage.setItem("superhet.cwPitch.v1", "banana");
    expect(loadCwPitch()).toBeNull();
    localStorage.setItem("superhet.cwPitch.v1", "9000");
    expect(loadCwPitch()).toBeNull();
  });
});

describe("wire format", () => {
  it("reads and writes cw_pitch_hz", () => {
    const raw = {
      id: 1,
      freq_hz: 7e6,
      mode: "cw",
      bandwidth_hz: 500,
      squelch_dbfs: null,
      deemphasis: null,
      cw_pitch_hz: 650,
      offset_hz: 0,
      status: "active",
    };
    expect(parseVfo(raw).cwPitchHz).toBe(650);
    expect(parseVfo({ ...raw, mode: "usb", cw_pitch_hz: undefined }).cwPitchHz).toBeNull();
    expect(patchBody({ cwPitchHz: 600 })).toEqual({ v: 1, cw_pitch_hz: 600 });
    expect(createBody({ freqHz: 1, mode: "cw", cwPitchHz: 600 })).toMatchObject({
      cw_pitch_hz: 600,
    });
  });
});

describe("CwPitchControl", () => {
  it("shows the VFO pitch and sends and remembers a change", () => {
    const onUpdate = vi.fn();
    render(<CwPitchControl vfo={cw({ cwPitchHz: 650 })} onUpdate={onUpdate} />);
    const slider = screen.getByRole("slider", { name: "Pitch" });
    expect(slider).toHaveValue("650");
    expect(screen.getByText("650 Hz")).toBeInTheDocument();
    fireEvent.change(slider, { target: { value: "800" } });
    expect(onUpdate).toHaveBeenCalledWith(1, { cwPitchHz: 800 });
    expect(loadCwPitch()).toBe(800);
  });

  it("falls back to 700 Hz when the daemon sent no pitch", () => {
    render(<CwPitchControl vfo={cw({ cwPitchHz: null })} onUpdate={vi.fn()} />);
    expect(screen.getByRole("slider", { name: "Pitch" })).toHaveValue("700");
  });
});

describe("VfoController", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  function setup(list: Vfo[]) {
    const updates: VfoPatch[] = [];
    const creates: VfoCreate[] = [];
    const reply = { vfo: cw(), vfos: [cw()], state: null, retune: "none" } as VfoReply;
    const api: VfoApi = {
      list: vi.fn(() => Promise.resolve(list)),
      create: vi.fn((_d: string, req: VfoCreate) => {
        creates.push(req);
        return Promise.resolve(reply);
      }),
      update: vi.fn((_d: string, _id: number, patch: VfoPatch) => {
        updates.push(patch);
        return Promise.resolve(reply);
      }),
      remove: vi.fn(() => Promise.resolve()),
    };
    const c = new VfoController({ device: "d", api, debounceMs: 10 });
    c.start();
    return { c, updates, creates };
  }

  it("starts a VFO switched to CW with the remembered pitch", async () => {
    saveCwPitch(600);
    const { c, updates } = setup([cw({ mode: "usb", cwPitchHz: null })]);
    await vi.advanceTimersByTimeAsync(0);
    c.update(1, { mode: "cw", bandwidthHz: null });
    await vi.advanceTimersByTimeAsync(20);
    expect(updates[0]).toMatchObject({ mode: "cw", cwPitchHz: 600 });
    c.stop();
  });

  it("does not override the pitch of a VFO that already is CW", async () => {
    saveCwPitch(600);
    const { c, updates } = setup([cw({ cwPitchHz: 750 })]);
    await vi.advanceTimersByTimeAsync(0);
    c.update(1, { mode: "cw", bandwidthHz: 300 });
    await vi.advanceTimersByTimeAsync(20);
    expect(updates[0]).not.toHaveProperty("cwPitchHz");
    c.stop();
  });

  it("creates CW VFOs with the remembered pitch unless one is given", async () => {
    saveCwPitch(600);
    const { c, creates } = setup([]);
    await vi.advanceTimersByTimeAsync(0);
    c.create({ freqHz: 7_030_000, mode: "cw" });
    await vi.advanceTimersByTimeAsync(20);
    c.create({ freqHz: 7_040_000, mode: "cw", cwPitchHz: 500 });
    await vi.advanceTimersByTimeAsync(20);
    c.create({ freqHz: 7_100_000, mode: "usb" });
    await vi.advanceTimersByTimeAsync(20);
    expect(creates.map((r) => r.cwPitchHz)).toEqual([600, 500, undefined]);
    c.stop();
  });
});
