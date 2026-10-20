// Audio controller: audio WebSocket + engine + per-VFO volume/mute + S-meter values.
// React reads the coarse `getSnapshot()`; per-frame values (meters, debug) are polled via getters
// at a low rate so no React render happens per audio frame.

import type { SocketStatus } from "../spectrum/socket";
import { AudioEngine, checkSupport, type EngineDebug } from "./engine";
import type { AudioFrame } from "./frame";
import { volumeToGain } from "./gain";
import { DEFAULT_TARGET_MS } from "./jitter";
import { AudioSocket, normalizeVfoIds } from "./socket";

export const DEFAULT_VOLUME = 0.8;

export interface VfoLevel {
  /** 0..1 slider position. */
  volume: number;
  muted: boolean;
}

export interface AudioSnapshot {
  /** null when the browser can decode Opus; otherwise the reason audio is unavailable. */
  unsupported: string | null;
  enabled: boolean;
  starting: boolean;
  error: string | null;
  socket: SocketStatus;
  targetMs: number;
  levels: Readonly<Record<number, VfoLevel>>;
}

export interface Meter {
  powerDbfs: number;
  squelchOpen: boolean;
  /** ms since the last frame. */
  ageMs: number;
}

export interface AudioControllerOptions {
  device: string;
  engine?: AudioEngine;
  createSocket?: (url: string) => WebSocket;
}

export class AudioController {
  private readonly engine: AudioEngine;
  private readonly socket: AudioSocket;
  private readonly meters = new Map<
    number,
    { powerDbfs: number; squelchOpen: boolean; at: number }
  >();
  private levels: Record<number, VfoLevel> = {};
  private ids: number[] = [];
  private snapshot: AudioSnapshot;
  private readonly listeners = new Set<() => void>();

  constructor(opts: AudioControllerOptions) {
    this.engine = opts.engine ?? new AudioEngine((m) => this.setError(m));
    const support = checkSupport();
    this.snapshot = {
      unsupported: support.ok ? null : (support.reason ?? "Audio is not supported."),
      enabled: false,
      starting: false,
      error: null,
      socket: "connecting",
      targetMs: DEFAULT_TARGET_MS,
      levels: this.levels,
    };
    this.socket = new AudioSocket({
      device: opts.device,
      createSocket: opts.createSocket,
      onFrame: (f) => this.onFrame(f),
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
    this.publish({ enabled: false, starting: false });
  }

  /** Subscribe to these VFOs (frames flow even with audio off, for the S-meter). */
  setVfos(ids: Iterable<number>): void {
    const next = normalizeVfoIds(ids);
    if (next.join(",") === this.ids.join(",")) return;
    this.ids = next;
    this.socket.setVfos(next);
    this.engine.setVfos(next);
    for (const id of [...this.meters.keys()]) if (!next.includes(id)) this.meters.delete(id);
    const levels: Record<number, VfoLevel> = {};
    for (const id of next) {
      levels[id] = this.levels[id] ?? { volume: DEFAULT_VOLUME, muted: false };
      this.engine.setGain(id, volumeToGain(levels[id].volume, levels[id].muted));
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

  setMuted(id: number, muted: boolean): void {
    this.setLevel(id, { muted });
  }

  setTargetMs(ms: number): void {
    this.engine.setTargetMs(ms);
    this.publish({ targetMs: ms });
  }

  getMeter(id: number, now = performance.now()): Meter | null {
    const m = this.meters.get(id);
    return m ? { powerDbfs: m.powerDbfs, squelchOpen: m.squelchOpen, ageMs: now - m.at } : null;
  }

  getDebug(): EngineDebug | null {
    return this.engine.debug();
  }

  private setLevel(id: number, change: Partial<VfoLevel>): void {
    const cur = this.levels[id] ?? { volume: DEFAULT_VOLUME, muted: false };
    const next = { ...cur, ...change };
    this.levels = { ...this.levels, [id]: next };
    this.engine.setGain(id, volumeToGain(next.volume, next.muted));
    this.publish({ levels: this.levels });
  }

  private onFrame(frame: AudioFrame): void {
    this.meters.set(frame.vfoId, {
      powerDbfs: frame.powerDbfs,
      squelchOpen: frame.squelchOpen,
      at: performance.now(),
    });
    this.engine.onFrame(frame);
  }

  private setError(error: string): void {
    this.publish({ error });
  }

  private publish(change: Partial<AudioSnapshot>): void {
    this.snapshot = { ...this.snapshot, ...change };
    for (const l of this.listeners) l();
  }
}
