// Fixtures for the shell component tests: VFOs, a device state and a fake receiver.

import { vi } from "vitest";
import type { AudioController } from "../audio/controller";
import type { PluginApi } from "../plugins/api";
import { PluginStore } from "../plugins/store";
import type { DeviceState, LockHolder } from "../spectrum/api";
import type { Vfo } from "../vfo/api";
import { AutoSquelch } from "../vfo/autoSquelch";
import type { Receiver } from "./useReceiver";

export const vfo = (id: number, over: Partial<Vfo> = {}): Vfo => ({
  id,
  freqHz: 100e6 + id * 1e5,
  mode: "nfm",
  bandwidthHz: 12_500,
  squelchDbfs: null,
  deemphasis: null,
  offsetHz: 0,
  status: "active",
  ...over,
});

export const device: DeviceState = {
  driver: "rtlsdr",
  centerHz: 100e6,
  sampleRate: 2_048_000,
  gain: { mode: "auto", db: null },
  gainsDb: [0, 10],
  ppm: 0,
  biasTee: false,
  tuningEpoch: 1,
  locks: [],
  window: { startHz: 99e6, endHz: 101e6 },
};

export const user: LockHolder = { kind: "user", id: "user", sinceMs: 1 };

export function fake(vfos: Vfo[], selectedId: number | null, locks: LockHolder[] = []) {
  const fns = {
    update: vi.fn(),
    select: vi.fn(),
    remove: vi.fn(),
    createVfoAt: vi.fn(),
    startBookmark: vi.fn(),
    setStep: vi.fn(),
    setListening: vi.fn(),
    toggleLock: vi.fn(),
    request: vi.fn(),
  };
  const state = { ...device, locks };
  const r = {
    device: "dev0",
    selected: vfos.find((v) => v.id === selectedId) ?? null,
    vfoSnap: { vfos, selectedId, busy: false, error: null, loaded: true },
    tuningSnap: { state, display: state, busy: false, lockBusy: false, error: null },
    audioSnap: { levels: { 1: { volume: 0.8, muted: false, listening: false } }, master: 1 },
    audio: {
      getMeter: () => null,
      getRecentLevels: () => [],
      setListening: fns.setListening,
    } as unknown as AudioController,
    autoSquelch: new AutoSquelch(),
    vfoCtl: { update: fns.update, select: fns.select, remove: fns.remove },
    tuning: { request: fns.request },
    locks,
    step: "auto",
    setStep: fns.setStep,
    createVfoAt: fns.createVfoAt,
    startBookmark: fns.startBookmark,
    toggleLock: fns.toggleLock,
    channelSnap: { prefs: { region: null, stepByChannel: true, rasters: {} } },
    channelMatch: null,
    channels: { setStepByChannel: vi.fn() },
    // Never started: the snapshot stays empty.
    plugins: new PluginStore({} as PluginApi),
    stepChannels: () => false,
  } as unknown as Receiver;
  return { r, fns };
}
