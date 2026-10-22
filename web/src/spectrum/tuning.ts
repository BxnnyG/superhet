// Tuning controller: coalesces tune requests (latest value per field wins, at most one request
// in flight) and keeps an optimistic display state. The confirmed state is pushed in by the state
// store (`ingest`); one REST read at start is the fallback if the socket is not up. It also owns
// the user tuning lock (take/drop) and refuses centre/rate changes while the lock is held.
// Plain TS; React subscribes through `subscribe`/`getSnapshot` (useSyncExternalStore).

import {
  ApiError,
  createApiClient,
  type ApiClient,
  type DeviceState,
  type LockHolder,
  type TunePatch,
} from "./api";

export interface TuningSnapshot {
  /** Last state confirmed by the device. */
  state: DeviceState | null;
  /** State to display: confirmed state with pending changes applied. */
  display: DeviceState | null;
  /** A change is pending or in flight. */
  busy: boolean;
  /** A lock/unlock request is in flight. */
  lockBusy: boolean;
  error: string | null;
}

export interface TuningOptions {
  device: string;
  api?: ApiClient;
  debounceMs?: number;
  /**
   * A centre/rate change was refused because the tuning is locked (by the client-side guard or
   * a 409). Without a handler the refusal is shown as the controller error.
   */
  onLocked?: (holders: LockHolder[], message: string) => void;
}

export function applyPatch(state: DeviceState, patch: TunePatch): DeviceState {
  const next = { ...state };
  if (patch.centerHz !== undefined) next.centerHz = patch.centerHz;
  if (patch.sampleRate !== undefined) next.sampleRate = patch.sampleRate;
  if (patch.ppm !== undefined) next.ppm = patch.ppm;
  if (patch.biasTee !== undefined) next.biasTee = patch.biasTee;
  if (patch.gain !== undefined && state.gain) {
    next.gain =
      patch.gain === "auto"
        ? { mode: "auto", db: state.gain.db }
        : { mode: "manual", db: patch.gain };
  }
  return next;
}

export class TuningController {
  private readonly api: ApiClient;
  private readonly debounceMs: number;
  private lockBusy = false;
  private snapshot: TuningSnapshot = {
    state: null,
    display: null,
    busy: false,
    lockBusy: false,
    error: null,
  };
  private pending: TunePatch = {};
  private hasPending = false;
  private inFlight = false;
  private sent: TunePatch = {};
  private debounceTimer: ReturnType<typeof setTimeout> | undefined;
  private polling = false;
  /** Bumped whenever a tune starts so older state reads are discarded. */
  private generation = 0;
  private stopped = true;
  private readonly listeners = new Set<() => void>();
  private abort: AbortController | null = null;

  constructor(private readonly opts: TuningOptions) {
    this.api = opts.api ?? createApiClient();
    this.debounceMs = opts.debounceMs ?? 50;
  }

  readonly subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  };

  readonly getSnapshot = (): TuningSnapshot => this.snapshot;

  start(): void {
    if (!this.stopped) return;
    this.stopped = false;
    void this.refresh();
  }

  stop(): void {
    this.stopped = true;
    clearTimeout(this.debounceTimer);
    this.debounceTimer = undefined;
    this.abort?.abort();
    this.abort = null;
  }

  /** Request a change; fields of earlier unsent requests are overwritten, others kept. */
  request(patch: TunePatch): void {
    const held = this.snapshot.state?.locks ?? [];
    if (held.length > 0) {
      const cur = this.snapshot.state;
      const centre = patch.centerHz !== undefined && patch.centerHz !== cur?.centerHz;
      const rate = patch.sampleRate !== undefined && patch.sampleRate !== cur?.sampleRate;
      if (centre || rate) {
        const rest = { ...patch };
        if (centre) delete rest.centerHz;
        if (rate) delete rest.sampleRate;
        this.rejectLocked(held, "tuning is locked");
        if (Object.keys(rest).length === 0) return;
        patch = rest;
      }
    }
    this.pending = { ...this.pending, ...patch };
    this.hasPending = true;
    this.publish({ error: null });
    if (!this.inFlight && this.debounceTimer === undefined) {
      this.debounceTimer = setTimeout(() => this.send(), this.debounceMs);
    }
  }

  /** Confirmed state pushed by the state store; ignored while a change is pending or in flight. */
  ingest(state: DeviceState): void {
    if (this.inFlight || this.hasPending) return;
    this.generation += 1;
    this.confirm(state, true);
  }

  /** Take (`true`) or drop (`false`) the user tuning lock. */
  async setLock(on: boolean): Promise<boolean> {
    if (this.lockBusy) return false;
    this.lockBusy = true;
    this.publish({});
    try {
      const state = on
        ? await this.api.lock(this.opts.device)
        : await this.api.unlock(this.opts.device);
      if (this.stopped) return false;
      this.generation += 1;
      this.lockBusy = false;
      this.publish({ state, error: null });
      return true;
    } catch (e) {
      if (this.stopped) return false;
      this.lockBusy = false;
      this.publish({ error: message(e) });
      return false;
    }
  }

  /** Read the device state unless a change is pending or in flight. */
  async refresh(keepError = false): Promise<void> {
    if (this.stopped || this.polling || this.inFlight || this.hasPending) return;
    this.polling = true;
    const generation = this.generation;
    const abort = new AbortController();
    this.abort = abort;
    try {
      const state = await this.api.getState(this.opts.device, abort.signal);
      if (this.stopped || generation !== this.generation || this.hasPending) return;
      this.confirm(state, keepError);
    } catch (e) {
      if (this.stopped || abort.signal.aborted) return;
      if (generation === this.generation && !this.hasPending) this.publish({ error: message(e) });
    } finally {
      this.polling = false;
    }
  }

  private send(): void {
    this.debounceTimer = undefined;
    if (this.stopped || this.inFlight || !this.hasPending) return;
    const patch = this.pending;
    this.pending = {};
    this.hasPending = false;
    this.sent = patch;
    this.inFlight = true;
    this.generation += 1;
    this.publish({});
    this.api
      .tune(this.opts.device, patch)
      .then((state) => {
        if (this.stopped) return;
        this.inFlight = false;
        this.sent = {};
        this.confirm(state);
      })
      .catch((e: unknown) => {
        if (this.stopped) return;
        this.inFlight = false;
        this.sent = {};
        // Drop unsent changes after an error; the display falls back to the device state.
        this.pending = {};
        this.hasPending = false;
        if (e instanceof ApiError && e.code === "locked" && this.opts.onLocked) {
          this.publish({});
          this.rejectLocked(e.holders ?? [], e.message);
        } else {
          this.publish({ error: message(e) });
        }
        void this.refresh(true);
      });
  }

  private rejectLocked(holders: LockHolder[], msg: string): void {
    if (this.opts.onLocked) this.opts.onLocked(holders, msg);
    else this.publish({ error: msg });
  }

  private confirm(state: DeviceState, keepError = false): void {
    this.publish(keepError ? { state } : { state, error: null });
    if (this.hasPending && this.debounceTimer === undefined) {
      this.debounceTimer = setTimeout(() => this.send(), this.debounceMs);
    }
  }

  private publish(change: Partial<Pick<TuningSnapshot, "state" | "error">>): void {
    const state = change.state ?? this.snapshot.state;
    const error = change.error === undefined ? this.snapshot.error : change.error;
    const display = state
      ? applyPatch(this.inFlight ? applyPatch(state, this.sent) : state, this.pending)
      : state;
    this.snapshot = {
      state,
      display,
      busy: this.inFlight || this.hasPending,
      lockBusy: this.lockBusy,
      error,
    };
    for (const l of this.listeners) l();
  }
}

function message(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}
