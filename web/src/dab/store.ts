// The DAB view's state as an external store. It follows one instance of the DAB plugin: the id
// comes from the tuning lock (a running plugin holds a `claim` on the device, visible to every
// role) or from the instance list (operators), and the plugin's own status document is polled
// once a second while an instance exists and the tab is visible. A block the user picked in the
// channel plan waits here as `pending` until the DAB tab asks for the takeover confirmation.

import { createEventApi, type EventApi } from "../decoders/api";
import { describeHttpError } from "../http";
import { createDabApi, type DabApi, type DabControl } from "./api";
import { DAB_PLUGIN_ID, latestEnsembles, type DabFound, type DabReply } from "./model";

export interface PendingBlock {
  channel: string;
  freqHz: number;
}

export interface DabSnapshot {
  /** The instance being followed; null when no DAB instance is known. */
  instanceId: string | null;
  /** Latest status reply; null before the first one or when there is no instance. */
  reply: DabReply | null;
  /** Why the status is unavailable (host down, instance gone); null when fine. */
  error: string | null;
  /** Ensembles found earlier on this device (decoder history), newest per block. */
  known: readonly DabFound[];
  /** A block picked in the channel plan that still needs confirmation. */
  pending: PendingBlock | null;
}

export const POLL_MS = 1000;

const EMPTY: DabSnapshot = { instanceId: null, reply: null, error: null, known: [], pending: null };

export class DabStore {
  private snap: DabSnapshot = EMPTY;
  private readonly listeners = new Set<() => void>();
  private timer: ReturnType<typeof setInterval> | undefined;
  private running = false;
  private abort: AbortController | null = null;
  private polling = false;

  constructor(
    private readonly device: string,
    private readonly api: DabApi = createDabApi(),
    private readonly events: EventApi = createEventApi(),
  ) {}

  readonly subscribe = (fn: () => void): (() => void) => {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  };

  readonly getSnapshot = (): DabSnapshot => this.snap;

  private set(next: Partial<DabSnapshot>): void {
    this.snap = { ...this.snap, ...next };
    for (const fn of [...this.listeners]) fn();
  }

  start(): void {
    if (this.running) return;
    this.running = true;
    this.abort = new AbortController();
    this.timer = setInterval(() => {
      if (typeof document === "undefined" || !document.hidden) void this.poll();
    }, POLL_MS);
    void this.poll();
    void this.loadKnown();
  }

  stop(): void {
    this.running = false;
    clearInterval(this.timer);
    this.abort?.abort();
    this.abort = null;
  }

  /**
   * Follow instance `id` (or none). Called whenever the claim holder or the instance list
   * changes; only a real change resets the shown reply.
   */
  follow(id: string | null): void {
    if (id === this.snap.instanceId) return;
    this.set({ instanceId: id, reply: null, error: null });
    if (id !== null && this.running) void this.poll();
  }

  /** A DAB block was picked in the channel plan: remember it for the confirmation dialog. */
  request(pending: PendingBlock | null): void {
    this.set({ pending });
  }

  /** Reload the ensembles found earlier (after a scan finished, when the view opens). */
  async loadKnown(): Promise<void> {
    try {
      const page = await this.events.list(
        this.device,
        { kinds: ["dab.ensemble"], source: DAB_PLUGIN_ID, q: "" },
        { limit: 400, signal: this.abort?.signal },
      );
      if (!this.running) return;
      this.set({ known: latestEnsembles(page.events) });
    } catch {
      // The history is a convenience; the live status works without it.
    }
  }

  private async poll(): Promise<void> {
    const id = this.snap.instanceId;
    if (id === null || this.polling) return;
    this.polling = true;
    try {
      const reply = await this.api.status(id, this.abort?.signal);
      if (this.snap.instanceId !== id) return;
      // Another plugin's claim (ADS-B) is none of ours.
      if (reply.pluginId !== DAB_PLUGIN_ID) {
        this.set({ instanceId: null, reply: null, error: null });
        return;
      }
      const wasScanning = this.snap.reply?.status?.mode === "scanning";
      this.set({ reply, error: null });
      if (wasScanning && reply.status?.mode !== "scanning") void this.loadKnown();
    } catch (e) {
      if (this.abort?.signal.aborted || this.snap.instanceId !== id) return;
      this.set({ error: describeHttpError(e) });
    } finally {
      this.polling = false;
    }
  }

  /** Send a control message to the followed instance (operators). */
  async control(message: DabControl): Promise<void> {
    const id = this.snap.instanceId;
    if (id === null) throw new Error("DAB is not running");
    await this.api.control(id, message);
    await this.poll();
  }
}
