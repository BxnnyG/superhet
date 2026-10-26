// VFO controller: keeps the device's VFO list, coalesces updates per VFO (latest value per
// field wins), sends at most one request at a time and reconciles with the daemon's replies.
// The confirmed list is pushed in by the state store (`ingest`); one REST read at start is the
// fallback if the socket is not up. Plain TS; React subscribes via `subscribe`/`getSnapshot`.

import { reportAccessError } from "../auth/accessErrors";
import type { DeviceState } from "../spectrum/api";
import { toasts } from "../ui/toast";
import {
  createVfoApi,
  type Retune,
  type Vfo,
  type VfoApi,
  type VfoCreate,
  type VfoPatch,
  type VfoReply,
} from "./api";
import { loadCwPitch } from "./cwPitch";
import { DEFAULT_BANDWIDTH_HZ } from "./model";

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
  onCreated?: (vfo: Vfo) => void;
  /** Free tuning outcome of a create/update that was not simply inside the window. */
  onRetune?: (result: Exclude<Retune, "none">, vfo: Vfo) => void;
  /** Device state carried by a create/update reply (the centre may have followed the VFO). */
  onState?: (state: DeviceState) => void;
}

/** What the viewer may do with VFOs (role, ownership, guest policy); set after creation. */
export interface VfoGuards {
  /**
   * Whether the viewer may change or delete this VFO (listeners and guests: only their own).
   * A refused change is not sent; a toast says why.
   */
  canModify?: (vfo: Vfo) => boolean;
  /** Reason why a new VFO must not be requested (known quota or guest policy), or null. */
  createBlock?: (req: VfoCreate, current: readonly Vfo[]) => string | null;
}

const NOT_MINE = {
  key: "not-owner",
  tone: "warn",
  title: "Not your receiver",
  body: "This receiver belongs to someone else. Only its owner, an operator or an admin can change it.",
} as const;

type Op = { kind: "create"; req: VfoCreate } | { kind: "delete"; id: number };

export function applyVfoPatch(vfo: Vfo, patch: VfoPatch): Vfo {
  const next = { ...vfo };
  if (patch.freqHz !== undefined) next.freqHz = patch.freqHz;
  if (patch.mode !== undefined) next.mode = patch.mode;
  if (patch.bandwidthHz != null) next.bandwidthHz = patch.bandwidthHz;
  // A mode change that resets the bandwidth shows the new mode's default right away.
  else if (patch.bandwidthHz === null && patch.mode !== undefined)
    next.bandwidthHz = DEFAULT_BANDWIDTH_HZ[patch.mode];
  if (patch.squelchDbfs !== undefined) next.squelchDbfs = patch.squelchDbfs;
  if (patch.cwPitchHz !== undefined) next.cwPitchHz = patch.cwPitchHz;
  return next;
}

export class VfoController {
  private readonly api: VfoApi;
  private readonly debounceMs: number;
  private guards: VfoGuards = {};
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
  private polling = false;
  private generation = 0;
  private stopped = true;
  private abort: AbortController | null = null;
  private readonly listeners = new Set<() => void>();

  constructor(private readonly opts: VfoControllerOptions) {
    this.api = opts.api ?? createVfoApi();
    this.debounceMs = opts.debounceMs ?? 50;
    this.snapshot = this.build();
  }

  /** Replace the permission guards (the signed-in role can change while the page runs). */
  setGuards(guards: VfoGuards): void {
    this.guards = guards;
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
  }

  stop(): void {
    this.stopped = true;
    clearTimeout(this.debounceTimer);
    this.debounceTimer = undefined;
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
    const block = this.guards.createBlock?.(req, this.snapshot.vfos);
    if (block) {
      toasts.push({
        key: "vfo-create",
        tone: "warn",
        title: "Cannot open a receiver",
        body: block,
      });
      return;
    }
    const pitch = req.mode === "cw" && req.cwPitchHz === undefined ? loadCwPitch() : null;
    this.ops.push({ kind: "create", req: pitch === null ? req : { ...req, cwPitchHz: pitch } });
    this.error = null;
    this.publish();
    this.schedule();
  }

  /** Change a VFO; fields of earlier unsent updates of the same VFO are overwritten. */
  update(id: number, patch: VfoPatch): void {
    if (!this.mayModify(id)) return;
    // Switching a VFO to CW starts with the pitch the user last picked (the daemon would
    // otherwise reset it to 700 Hz).
    const current = this.confirmed.find((v) => v.id === id);
    const pitch =
      patch.mode === "cw" && patch.cwPitchHz === undefined && current?.mode !== "cw"
        ? loadCwPitch()
        : null;
    this.pending.set(id, {
      ...this.pending.get(id),
      ...patch,
      ...(pitch === null ? {} : { cwPitchHz: pitch }),
    });
    this.error = null;
    this.publish();
    this.schedule();
  }

  remove(id: number): boolean {
    if (!this.mayModify(id)) return false;
    this.pending.delete(id);
    this.ops.push({ kind: "delete", id });
    if (this.selectedId === id) this.selectedId = null;
    this.error = null;
    this.publish();
    this.schedule();
    return true;
  }

  private mayModify(id: number): boolean {
    const vfo = this.confirmed.find((v) => v.id === id);
    if (!vfo || !this.guards.canModify || this.guards.canModify(vfo)) return true;
    toasts.push(NOT_MINE);
    return false;
  }

  /** Confirmed list pushed by the state store; ignored while a change is pending or in flight. */
  ingest(vfos: Vfo[]): void {
    if (this.isBusy()) return;
    this.generation += 1;
    this.setConfirmed(vfos);
    this.publish();
  }

  /** Read the list unless a change is pending/in flight. */
  async refresh(keepError = false): Promise<void> {
    if (this.stopped || this.polling || this.inFlight || this.isBusy()) return;
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
        this.afterReply(reply);
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
        this.afterReply(reply);
      }
    } catch (e) {
      if (this.stopped) return;
      failed = true;
      // A refusal by role, ownership or guest policy is a toast, not a red banner.
      this.error = reportAccessError(e) ? null : message(e);
      // Drop unsent changes of the failed VFO; the display falls back to the device state.
      if (patchEntry) this.pending.delete(patchEntry[0]);
    } finally {
      if (!this.stopped) {
        this.inFlight = false;
        this.sent = null;
        this.sentDelete = null;
        this.publish();
        if (failed) void this.refresh(true);
        if (this.isBusy()) this.schedule();
      }
    }
  }

  private afterReply(reply: VfoReply): void {
    if (reply.state) this.opts.onState?.(reply.state);
    if (reply.retune !== "none") this.opts.onRetune?.(reply.retune, reply.vfo);
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
