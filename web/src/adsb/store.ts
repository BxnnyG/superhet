// The ADS-B view's state as an external store. It follows one instance of the ADS-B plugin: the id
// comes from the instance list (operators) or from the plugin's claim on the device (visible to
// every role, like the DAB view), and the plugin's status document is polled once a second while
// an instance exists and the tab is visible. The map and table overlay (`viewOpen`) is opened
// from the inspector tab.

import { describeHttpError } from "../http";
import { createAdsbApi, type AdsbApi } from "./api";
import { InfoCache } from "./info";
import { ADSB_PLUGIN_ID, type AdsbReply } from "./model";
import { TrailStore } from "./trails";

export interface AdsbSnapshot {
  /** The instance being followed; null when no ADS-B instance is known. */
  instanceId: string | null;
  /** Latest status reply; null before the first one or when there is no instance. */
  reply: AdsbReply | null;
  /** When the latest reply arrived (ms since epoch), for ageing aircraft between polls. */
  receivedAt: number;
  /** Why the status is unavailable (host down, instance gone); null when fine. */
  error: string | null;
  /** The map and table overlay is open. */
  viewOpen: boolean;
}

export const POLL_MS = 1000;

const EMPTY: AdsbSnapshot = {
  instanceId: null,
  reply: null,
  receivedAt: 0,
  error: null,
  viewOpen: false,
};

export class AdsbStore {
  private snap: AdsbSnapshot = EMPTY;
  private readonly listeners = new Set<() => void>();
  private timer: ReturnType<typeof setInterval> | undefined;
  private running = false;
  private abort: AbortController | null = null;
  private polling = false;

  /** Registration, type and airline of the aircraft in view (the imported database). */
  readonly info: InfoCache;
  /** Where each aircraft has been (the plugin only reports where it is now). */
  readonly trails = new TrailStore();

  constructor(readonly client: AdsbApi = createAdsbApi()) {
    this.info = new InfoCache(client);
  }

  readonly subscribe = (fn: () => void): (() => void) => {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  };

  readonly getSnapshot = (): AdsbSnapshot => this.snap;

  private set(next: Partial<AdsbSnapshot>): void {
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
    this.trails.clear();
    this.set({ instanceId: id, reply: null, receivedAt: 0, error: null });
    if (id !== null && this.running) void this.poll();
  }

  openView(): void {
    if (!this.snap.viewOpen) this.set({ viewOpen: true });
  }

  closeView(): void {
    if (this.snap.viewOpen) this.set({ viewOpen: false });
  }

  private async poll(): Promise<void> {
    const id = this.snap.instanceId;
    if (id === null || this.polling) return;
    this.polling = true;
    try {
      const reply = await this.client.status(id, this.abort?.signal);
      if (this.snap.instanceId !== id) return;
      // Another plugin's claim (DAB) is none of ours.
      if (reply.pluginId !== ADSB_PLUGIN_ID) {
        this.set({ instanceId: null, reply: null, receivedAt: 0, error: null });
        return;
      }
      const receivedAt = Date.now();
      this.trails.record(reply.status?.aircraft ?? [], receivedAt);
      this.set({ reply, receivedAt, error: null });
    } catch (e) {
      if (this.abort?.signal.aborted || this.snap.instanceId !== id) return;
      this.set({ error: describeHttpError(e) });
    } finally {
      this.polling = false;
    }
  }
}
