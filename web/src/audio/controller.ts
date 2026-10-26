// Audio controller: audio WebSocket + engine + per-VFO volume/mute + S-meter values.
// React reads the coarse `getSnapshot()`; per-frame values (meters, debug) are polled via getters
// at a low rate so no React render happens per audio frame.

import type { SocketStatus } from "../spectrum/socket";
import { AudioEngine, checkSupport, type EngineDebug } from "./engine";
import type { AudioFrame } from "./frame";
import { volumeToGain } from "./gain";
import { DEFAULT_TARGET_MS } from "./jitter";
import { loadSilent, saveSilent } from "./listen";
import { clampMaster, loadMaster, saveMaster } from "./master";
import { isEmptyRds, sameRds, type RdsState } from "./rds";
import { AudioSocket, normalizeVfoIds } from "./socket";
import { applyTones, isEmptyTones, sameTones, type ToneState, type TonesMessage } from "./tones";

export const DEFAULT_VOLUME = 0.8;

export interface VfoLevel {
  /** 0..1 slider position. */
  volume: number;
  muted: boolean;
  /** The VFO is part of the audio mix (independent of which VFO is active). */
  listening: boolean;
}

export interface AudioSnapshot {
  /** null when the browser can decode Opus; otherwise the reason audio is unavailable. */
  unsupported: string | null;
  enabled: boolean;
  starting: boolean;
  error: string | null;
  socket: SocketStatus;
  targetMs: number;
  /** Master output volume, 0..1, on top of the per-VFO volumes. */
  master: number;
  levels: Readonly<Record<number, VfoLevel>>;
  /** Latest RDS state of the VFOs that have any. */
  rds: Readonly<Record<number, RdsState>>;
  /** CTCSS tone and DTMF digits of the NFM / AM VFOs that have any. */
  tones: Readonly<Record<number, ToneState>>;
}

function withoutRds(rds: Record<number, RdsState>, id: number): Record<number, RdsState> {
  return Object.fromEntries(Object.entries(rds).filter(([key]) => Number(key) !== id));
}

function withoutVfo<T>(map: Record<number, T>, id: number): Record<number, T> {
  return Object.fromEntries(Object.entries(map).filter(([key]) => Number(key) !== id));
}

export interface Meter {
  powerDbfs: number;
  squelchOpen: boolean;
  /** Stereo pilot locked (the audio is genuinely stereo). */
  stereo: boolean;
  /** ms since the last frame. */
  ageMs: number;
}

/** How long channel-level samples are kept for the squelch "Auto" computation. */
const HISTORY_MS = 5000;

export interface AudioControllerOptions {
  device: string;
  /** Storage for the per-device listen set (default: localStorage). */
  storage?: Storage | null;
  engine?: AudioEngine;
  createSocket?: (url: string) => WebSocket;
}

export class AudioController {
  private readonly engine: AudioEngine;
  private readonly socket: AudioSocket;
  private readonly meters = new Map<
    number,
    { powerDbfs: number; squelchOpen: boolean; stereo: boolean; at: number }
  >();
  private rds: Record<number, RdsState> = {};
  private tones: Record<number, ToneState> = {};
  private readonly frequencies = new Map<number, number>();
  private readonly history = new Map<number, { powerDbfs: number; at: number }[]>();
  private levels: Record<number, VfoLevel> = {};
  private ids: number[] = [];
  private activeId: number | null = null;
  private readonly silent: Set<number>;
  private master: number;
  private readonly storage: Storage | null | undefined;
  private readonly device: string;
  private subscribed = new Set<number>();
  private listening = new Set<number>();
  private snapshot: AudioSnapshot;
  private readonly listeners = new Set<() => void>();

  constructor(opts: AudioControllerOptions) {
    this.device = opts.device;
    this.storage = opts.storage;
    this.silent = loadSilent(opts.device, opts.storage);
    this.master = loadMaster(opts.storage);
    this.engine = opts.engine ?? new AudioEngine((m) => this.setError(m));
    const support = checkSupport();
    this.snapshot = {
      unsupported: support.ok ? null : (support.reason ?? "Audio is not supported."),
      enabled: false,
      starting: false,
      error: null,
      socket: "connecting",
      targetMs: DEFAULT_TARGET_MS,
      master: this.master,
      levels: this.levels,
      rds: this.rds,
      tones: this.tones,
    };
    this.socket = new AudioSocket({
      device: opts.device,
      createSocket: opts.createSocket,
      onFrame: (f) => this.onFrame(f),
      onRds: (id, state) => this.onRds(id, state),
      onTones: (msg) => this.onTones(msg),
      onStatus: (socket) => this.publish({ socket }),
      onError: (m) => this.setError(m),
    });
  }

  readonly subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  };

  readonly getSnapshot = (): AudioSnapshot => this.snapshot;

  start(): void {
    this.socket.start();
  }

  stop(): void {
    this.socket.stop();
    void this.engine.disable();
    this.meters.clear();
    this.history.clear();
    this.rds = {};
    this.tones = {};
    this.publish({ enabled: false, starting: false, rds: this.rds, tones: this.tones });
  }

  /**
   * The VFOs that exist. VFOs that are not listened to get no audio stream; the active VFO is
   * still subscribed so its S-meter and squelch "Auto" keep working.
   */
  setVfos(ids: Iterable<number>): void {
    const next = normalizeVfoIds(ids);
    if (next.join(",") === this.ids.join(",")) return;
    this.ids = next;
    // Forget listen settings of VFOs that are gone (ids may be reused).
    let pruned = false;
    for (const id of [...this.silent]) {
      if (!next.includes(id)) {
        this.silent.delete(id);
        pruned = true;
      }
    }
    if (pruned) saveSilent(this.device, this.silent, this.storage);
    this.apply();
  }

  /** The VFO whose meter must keep running even when it is not listened to. */
  setActive(id: number | null): void {
    if (this.activeId === id) return;
    this.activeId = id;
    this.apply();
  }

  /** Add the VFO to or remove it from the audio mix; persisted per device. */
  setListening(id: number, on: boolean): void {
    if (on === !this.silent.has(id)) return;
    if (on) this.silent.delete(id);
    else this.silent.add(id);
    saveSilent(this.device, this.silent, this.storage);
    this.apply();
  }

  private apply(): void {
    const known = this.ids;
    const listening = new Set(known.filter((id) => !this.silent.has(id)));
    const subscribed = new Set(listening);
    if (this.activeId !== null && known.includes(this.activeId)) subscribed.add(this.activeId);
    this.listening = listening;
    this.subscribed = subscribed;
    this.socket.setVfos(subscribed);
    this.engine.setVfos(listening);
    for (const id of [...this.meters.keys()]) if (!subscribed.has(id)) this.meters.delete(id);
    for (const id of [...this.history.keys()]) if (!subscribed.has(id)) this.history.delete(id);
    const stale = Object.keys(this.rds)
      .map(Number)
      .filter((id) => !subscribed.has(id));
    if (stale.length > 0) {
      this.rds = Object.fromEntries(
        Object.entries(this.rds).filter(([id]) => subscribed.has(Number(id))),
      );
      this.publish({ rds: this.rds });
    }
    if (Object.keys(this.tones).some((id) => !subscribed.has(Number(id)))) {
      this.tones = Object.fromEntries(
        Object.entries(this.tones).filter(([id]) => subscribed.has(Number(id))),
      );
      this.publish({ tones: this.tones });
    }
    const levels: Record<number, VfoLevel> = {};
    for (const id of known) {
      const cur = this.levels[id];
      const level = {
        volume: cur?.volume ?? DEFAULT_VOLUME,
        muted: cur?.muted ?? false,
        listening: listening.has(id),
      };
      levels[id] = level;
      if (level.listening) this.engine.setGain(id, this.gainOf(level));
    }
    this.levels = levels;
    this.publish({ levels });
  }

  /** Must run inside a user gesture (click handler). */
  async enable(): Promise<void> {
    if (this.snapshot.unsupported) return;
    this.publish({ starting: true, error: null });
    try {
      await this.engine.enable();
      this.publish({ enabled: true, starting: false });
    } catch (e) {
      this.publish({
        enabled: false,
        starting: false,
        error: e instanceof Error ? e.message : String(e),
      });
    }
  }

  async disable(): Promise<void> {
    await this.engine.disable();
    this.publish({ enabled: false });
  }

  setVolume(id: number, volume: number): void {
    this.setLevel(id, { volume: Math.min(1, Math.max(0, volume)) });
  }

  /** Master output volume (0..1); scales every VFO and is persisted in this browser. */
  setMasterVolume(volume: number): void {
    const next = clampMaster(volume);
    if (next === this.master) return;
    this.master = next;
    saveMaster(next, this.storage);
    for (const [key, level] of Object.entries(this.levels)) {
      if (level.listening) this.engine.setGain(Number(key), this.gainOf(level));
    }
    this.publish({ master: next });
  }

  setMuted(id: number, muted: boolean): void {
    this.setLevel(id, { muted });
  }

  setTargetMs(ms: number): void {
    this.engine.setTargetMs(ms);
    this.publish({ targetMs: ms });
  }

  getMeter(id: number, now = performance.now()): Meter | null {
    const m = this.meters.get(id);
    return m
      ? { powerDbfs: m.powerDbfs, squelchOpen: m.squelchOpen, stereo: m.stereo, ageMs: now - m.at }
      : null;
  }

  /** Channel powers (dBFS) received for a VFO within the last `windowMs`, oldest first. */
  getRecentLevels(id: number, windowMs: number, now = performance.now()): number[] {
    const h = this.history.get(id);
    if (!h) return [];
    return h.filter((s) => now - s.at <= windowMs).map((s) => s.powerDbfs);
  }

  getDebug(): EngineDebug | null {
    return this.engine.debug();
  }

  /** Linear gain of a VFO: its own volume curve times the master curve. */
  private gainOf(level: VfoLevel): number {
    return volumeToGain(level.volume, level.muted) * volumeToGain(this.master);
  }

  private setLevel(id: number, change: Partial<VfoLevel>): void {
    const cur = this.levels[id] ?? {
      volume: DEFAULT_VOLUME,
      muted: false,
      listening: !this.silent.has(id),
    };
    const next = { ...cur, ...change };
    this.levels = { ...this.levels, [id]: next };
    this.engine.setGain(id, this.gainOf(next));
    this.publish({ levels: this.levels });
  }

  private onFrame(frame: AudioFrame): void {
    if (!this.subscribed.has(frame.vfoId)) return;
    const at = performance.now();
    this.meters.set(frame.vfoId, {
      powerDbfs: frame.powerDbfs,
      squelchOpen: frame.squelchOpen,
      stereo: frame.stereo,
      at,
    });
    let h = this.history.get(frame.vfoId);
    if (!h) this.history.set(frame.vfoId, (h = []));
    h.push({ powerDbfs: frame.powerDbfs, at });
    let drop = 0;
    while (drop < h.length && at - (h[drop]?.at ?? at) > HISTORY_MS) drop++;
    if (drop > 0) h.splice(0, drop);
    if (this.listening.has(frame.vfoId)) this.engine.onFrame(frame);
  }

  private onRds(id: number, state: RdsState): void {
    if (!this.subscribed.has(id)) return;
    const cur = this.rds[id];
    if (isEmptyRds(state)) {
      if (!cur) return;
      this.rds = withoutRds(this.rds, id);
    } else {
      if (cur && sameRds(cur, state)) return;
      this.rds = { ...this.rds, [id]: state };
    }
    this.publish({ rds: this.rds });
  }

  private onTones(msg: TonesMessage): void {
    const id = msg.vfo;
    if (!this.subscribed.has(id)) return;
    const cur = this.tones[id];
    if (isEmptyTones(msg)) {
      if (!cur) return;
      this.tones = withoutVfo(this.tones, id);
    } else {
      const next = applyTones(cur, msg);
      if (cur && sameTones(cur, next)) return;
      this.tones = { ...this.tones, [id]: next };
    }
    this.publish({ tones: this.tones });
  }

  /** Forget the DTMF digits shown for a VFO (the daemon keeps counting; nothing comes back). */
  clearDigits(id: number): void {
    const cur = this.tones[id];
    if (!cur || cur.digits === "") return;
    this.tones = { ...this.tones, [id]: { ...cur, digits: "" } };
    this.publish({ tones: this.tones });
  }

  /**
   * Tell the controller where a VFO is tuned. A retune by more than 5 kHz means another station:
   * its RDS and tone data are dropped at once instead of waiting for the daemon's clear message.
   */
  noteFrequency(id: number, freqHz: number): void {
    const prev = this.frequencies.get(id);
    this.frequencies.set(id, freqHz);
    if (prev === undefined || Math.abs(prev - freqHz) <= 5000) return;
    const change: Partial<AudioSnapshot> = {};
    if (this.rds[id]) {
      this.rds = withoutRds(this.rds, id);
      change.rds = this.rds;
    }
    if (this.tones[id]) {
      this.tones = withoutVfo(this.tones, id);
      change.tones = this.tones;
    }
    if (change.rds || change.tones) this.publish(change);
  }

  private setError(error: string): void {
    this.publish({ error });
  }

  private publish(change: Partial<AudioSnapshot>): void {
    this.snapshot = { ...this.snapshot, ...change };
    for (const l of this.listeners) l();
  }
}
