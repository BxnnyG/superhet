// VFO controller: keeps the device's VFO list, coalesces updates per VFO (latest value per
// field wins), sends at most one request at a time and reconciles with the daemon's replies
// and a slow poll. Plain TS; React subscribes via `subscribe`/`getSnapshot`.

import { createVfoApi, type Vfo, type VfoApi, type VfoCreate, type VfoPatch } from "./api";

export interface VfoSnapshot {
  /** Confirmed VFOs with in-flight and pending changes applied. */
  vfos: Vfo[];
  selectedId: number | null;
  /** A change is pending or in flight. */
  busy: boolean;
  error: string | null;
  /** The list has been read from the device at least once. */
  loaded: boolean;
}

export interface VfoControllerOptions {
  device: string;
  api?: VfoApi;
  debounceMs?: number;
  pollMs?: number;
  /** Polls are skipped this long after a change (GET /vfos may lag the daemon). */
  quietMs?: number;
  onCreated?: (vfo: Vfo) => void;
}

type Op = { kind: "create"; req: VfoCreate } | { kind: "delete"; id: number };

export function applyVfoPatch(vfo: Vfo, patch: VfoPatch): Vfo {
  const next = { ...vfo };
  if (patch.freqHz !== undefined) next.freqHz = patch.freqHz;
  if (patch.mode !== undefined) next.mode = patch.mode;
  if (patch.bandwidthHz != null) next.bandwidthHz = patch.bandwidthHz;
  if (patch.squelchDbfs !== undefined) next.squelchDbfs = patch.squelchDbfs;
  return next;
}

export class VfoController {
  private readonly api: VfoApi;
  private readonly debounceMs: number;
  private readonly pollMs: number;
  private readonly quietMs: number;
  private confirmed: Vfo[] = [];
  private loaded = false;
  private selectedId: number | null = null;
  private error: string | null = null;
  private pending = new Map<number, VfoPatch>();
  private ops: Op[] = [];
  private inFlight = false;
  private sent: { id: number; patch: VfoPatch } | null = null;
  private sentDelete: number | null = null;
  private snapshot: VfoSnapshot;
  private debounceTimer: ReturnType<typeof setTimeout> | undefined;
  private pollTimer: ReturnType<typeof setInterval> | undefined;
  private polling = false;
  private generation = 0;
  private quietUntil = 0;
  private stopped = true;
  private abort: AbortController | null = null;
  private readonly listeners = new Set<() => void>();

  constructor(private readonly opts: VfoControllerOptions) {
    this.api = opts.api ?? createVfoApi();
    this.debounceMs = opts.debounceMs ?? 50;
    this.pollMs = opts.pollMs ?? 2000;
    this.quietMs = opts.quietMs ?? 1000;
    this.snapshot = this.build();
  }

  readonly subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  };

  readonly getSnapshot = (): VfoSnapshot => this.snapshot;

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

  select(id: number | null): void {
    if (this.selectedId === id) return;
    this.selectedId = id;
    this.publish();
  }

  /** Create a VFO; it appears (and is selected) when the daemon has answered. */
  create(req: VfoCreate): void {
    this.ops.push({ kind: "create", req });
    this.error = null;
    this.publish();
    this.schedule();
  }

  /** Change a VFO; fields of earlier unsent updates of the same VFO are overwritten. */
  update(id: number, patch: VfoPatch): void {
    this.pending.set(id, { ...this.pending.get(id), ...patch });
    this.error = null;
    this.publish();
    this.schedule();
  }

  remove(id: number): void {
    this.pending.delete(id);
    this.ops.push({ kind: "delete", id });
    if (this.selectedId === id) this.selectedId = null;
    this.error = null;
    this.publish();
    this.schedule();
  }

  /** Read the list unless a change is pending/in flight or one just completed. */
  async refresh(force = false, keepError = false): Promise<void> {
    if (this.stopped || this.polling || this.inFlight || this.isBusy()) return;
    if (!force && Date.now() < this.quietUntil) return;
    this.polling = true;
    const generation = this.generation;
    const abort = new AbortController();
    this.abort = abort;
    try {
      const vfos = await this.api.list(this.opts.device, abort.signal);
      if (this.stopped || generation !== this.generation || this.isBusy()) return;
      this.setConfirmed(vfos);
      if (!keepError) this.error = null;
      this.publish();
    } catch (e) {
      if (this.stopped || abort.signal.aborted) return;
      if (generation === this.generation && !this.isBusy()) {
        this.error = message(e);
        this.publish();
      }
    } finally {
      this.polling = false;
    }
  }

  private isBusy(): boolean {
    return this.inFlight || this.pending.size > 0 || this.ops.length > 0;
  }

  private schedule(): void {
    if (this.stopped || this.inFlight || this.debounceTimer !== undefined) return;
    this.debounceTimer = setTimeout(() => void this.send(), this.debounceMs);
  }

  private async send(): Promise<void> {
    this.debounceTimer = undefined;
    if (this.stopped || this.inFlight) return;
    const op = this.ops.shift();
    let patchEntry: [number, VfoPatch] | undefined;
    if (!op) {
      patchEntry = this.pending.entries().next().value;
      if (!patchEntry) return;
      this.pending.delete(patchEntry[0]);
    }
    this.inFlight = true;
    this.generation += 1;
    if (op?.kind === "delete") this.sentDelete = op.id;
    if (patchEntry) this.sent = { id: patchEntry[0], patch: patchEntry[1] };
    this.publish();
    let failed = false;
    try {
      if (op?.kind === "create") {
        const reply = await this.api.create(this.opts.device, op.req);
        if (this.stopped) return;
        this.setConfirmed(reply.vfos ?? [...this.confirmed, reply.vfo]);
        this.selectedId = reply.vfo.id;
        this.opts.onCreated?.(reply.vfo);
      } else if (op?.kind === "delete") {
        await this.api.remove(this.opts.device, op.id);
        if (this.stopped) return;
        this.setConfirmed(this.confirmed.filter((v) => v.id !== op.id));
      } else if (patchEntry) {
        const reply = await this.api.update(this.opts.device, patchEntry[0], patchEntry[1]);
        if (this.stopped) return;
        this.setConfirmed(
          reply.vfos ?? this.confirmed.map((v) => (v.id === reply.vfo.id ? reply.vfo : v)),
        );
      }
    } catch (e) {
      if (this.stopped) return;
      failed = true;
      this.error = message(e);
      // Drop unsent changes of the failed VFO; the display falls back to the device state.
      if (patchEntry) this.pending.delete(patchEntry[0]);
    } finally {
      if (!this.stopped) {
        this.inFlight = false;
        this.sent = null;
        this.sentDelete = null;
        this.quietUntil = Date.now() + this.quietMs;
        this.publish();
        if (failed) void this.refresh(true, true);
        if (this.isBusy()) this.schedule();
      }
    }
  }

  private setConfirmed(vfos: Vfo[]): void {
    this.confirmed = [...vfos].sort((a, b) => a.id - b.id);
    this.loaded = true;
    if (this.selectedId !== null && !vfos.some((v) => v.id === this.selectedId)) {
      this.selectedId = null;
    }
  }

  private build(): VfoSnapshot {
    const removing = new Set<number>(this.sentDelete === null ? [] : [this.sentDelete]);
    for (const op of this.ops) if (op.kind === "delete") removing.add(op.id);
    const vfos = this.confirmed
      .filter((v) => !removing.has(v.id))
      .map((v) => {
        let out = v;
        if (this.sent?.id === v.id) out = applyVfoPatch(out, this.sent.patch);
        const p = this.pending.get(v.id);
        return p ? applyVfoPatch(out, p) : out;
      });
    return {
      vfos,
      selectedId: this.selectedId,
      busy: this.isBusy(),
      error: this.error,
      loaded: this.loaded,
    };
  }

  private publish(): void {
    this.snapshot = this.build();
    for (const l of this.listeners) l();
  }
}

function message(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}
