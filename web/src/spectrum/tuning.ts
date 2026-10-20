// Tuning controller: coalesces tune requests (latest value per field wins, at most one request
// in flight), keeps an optimistic display state and polls the device state while idle.
// Plain TS; React subscribes through `subscribe`/`getSnapshot` (useSyncExternalStore).

import { createApiClient, type ApiClient, type DeviceState, type TunePatch } from "./api";

export interface TuningSnapshot {
  /** Last state confirmed by the device. */
  state: DeviceState | null;
  /** State to display: confirmed state with pending changes applied. */
  display: DeviceState | null;
  /** A change is pending or in flight. */
  busy: boolean;
  error: string | null;
}

export interface TuningOptions {
  device: string;
  api?: ApiClient;
  debounceMs?: number;
  pollMs?: number;
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
  private readonly pollMs: number;
  private snapshot: TuningSnapshot = { state: null, display: null, busy: false, error: null };
  private pending: TunePatch = {};
  private hasPending = false;
  private inFlight = false;
  private sent: TunePatch = {};
  private debounceTimer: ReturnType<typeof setTimeout> | undefined;
  private pollTimer: ReturnType<typeof setInterval> | undefined;
  private polling = false;
  /** Bumped whenever a tune starts so older state reads are discarded. */
  private generation = 0;
  private stopped = true;
  private readonly listeners = new Set<() => void>();
  private abort: AbortController | null = null;

  constructor(private readonly opts: TuningOptions) {
    this.api = opts.api ?? createApiClient();
    this.debounceMs = opts.debounceMs ?? 50;
    this.pollMs = opts.pollMs ?? 5000;
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
    this.pollTimer = setInterval(() => void this.refresh(), this.pollMs);
  }

  stop(): void {
    this.stopped = true;
    clearTimeout(this.debounceTimer);
    this.debounceTimer = undefined;
    clearInterval(this.pollTimer);
    this.abort?.abort();
    this.abort = null;
  }

  /** Request a change; fields of earlier unsent requests are overwritten, others kept. */
  request(patch: TunePatch): void {
    this.pending = { ...this.pending, ...patch };
    this.hasPending = true;
    this.publish({ error: null });
    if (!this.inFlight && this.debounceTimer === undefined) {
      this.debounceTimer = setTimeout(() => this.send(), this.debounceMs);
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
        this.publish({ error: message(e) });
        void this.refresh(true);
      });
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
      error,
    };
    for (const l of this.listeners) l();
  }
}

function message(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}
