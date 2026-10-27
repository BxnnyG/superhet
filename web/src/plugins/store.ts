// Installed plugins and their instances as an external store for the receiver UI (operators and
// admins; listeners and guests never start it). The host has no push channel for instance state
// yet (docs/api-plugins.md: "poll"), so the store polls every few seconds while it is running and
// the tab is visible; start/stop/restart refresh at once.

import { describeHttpError } from "../http";
import {
  createPluginApi,
  type PluginApi,
  type PluginInstance,
  type PluginManifest,
  type StartRequest,
} from "./api";

export interface PluginSnapshot {
  plugins: readonly PluginManifest[];
  instances: readonly PluginInstance[];
  /** The first answer (success or failure) has arrived. */
  loaded: boolean;
  /** Why the lists are unavailable (host not running, no bus, ...); null when fine. */
  error: string | null;
}

export const POLL_MS = 3000;

const EMPTY: PluginSnapshot = { plugins: [], instances: [], loaded: false, error: null };

export class PluginStore {
  private snap: PluginSnapshot = EMPTY;
  private readonly listeners = new Set<() => void>();
  private timer: ReturnType<typeof setInterval> | undefined;
  private running = false;
  private abort: AbortController | null = null;

  constructor(private readonly api: PluginApi = createPluginApi()) {}

  readonly subscribe = (fn: () => void): (() => void) => {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  };

  readonly getSnapshot = (): PluginSnapshot => this.snap;

  private set(next: Partial<PluginSnapshot>): void {
    this.snap = { ...this.snap, ...next };
    for (const fn of [...this.listeners]) fn();
  }

  start(): void {
    if (this.running) return;
    this.running = true;
    this.abort = new AbortController();
    void this.refresh();
    this.timer = setInterval(() => {
      if (typeof document === "undefined" || !document.hidden) void this.refreshInstances();
    }, POLL_MS);
  }

  stop(): void {
    this.running = false;
    clearInterval(this.timer);
    this.abort?.abort();
    this.abort = null;
  }

  /** Reload plugins and instances. */
  async refresh(): Promise<void> {
    const signal = this.abort?.signal;
    try {
      const [plugins, instances] = await Promise.all([
        this.api.listPlugins(signal),
        this.api.listInstances(signal),
      ]);
      if (signal?.aborted) return;
      this.set({ plugins, instances, loaded: true, error: null });
    } catch (e) {
      if (signal?.aborted) return;
      this.set({ loaded: true, error: describeHttpError(e) });
    }
  }

  private async refreshInstances(): Promise<void> {
    const signal = this.abort?.signal;
    try {
      const instances = await this.api.listInstances(signal);
      if (signal?.aborted) return;
      // A recovering host also brings the plugin list back.
      if (this.snap.error !== null) await this.refresh();
      else this.set({ instances });
    } catch (e) {
      if (signal?.aborted) return;
      this.set({ error: describeHttpError(e) });
    }
  }

  /** Start an instance; rejects with the server's message (conflict, invalid config, ...). */
  async startInstance(req: StartRequest): Promise<PluginInstance> {
    const created = await this.api.start(req);
    await this.refreshInstances();
    return created;
  }

  async stopInstance(id: string): Promise<void> {
    await this.api.stop(id);
    await this.refreshInstances();
  }

  async restartInstance(id: string): Promise<void> {
    await this.api.restart(id);
    await this.refreshInstances();
  }
}

// ---------- selectors ----------

/** Instances fed by one VFO of a device. */
export function instancesOnVfo(
  instances: readonly PluginInstance[],
  device: string,
  vfo: number,
): PluginInstance[] {
  return instances.filter((i) => i.device === device && i.vfo === vfo);
}

/** Instances that are bound to the device as a whole (exclusive plugins, informational ones). */
export function instancesOnDevice(
  instances: readonly PluginInstance[],
  device: string,
): PluginInstance[] {
  return instances.filter((i) => i.device === device && i.vfo === null);
}

/** Display name of a plugin id: the manifest name, else the last id segment. */
export function pluginName(plugins: readonly PluginManifest[], id: string): string {
  return plugins.find((p) => p.id === id)?.name ?? id.split(".").slice(-1)[0] ?? id;
}
