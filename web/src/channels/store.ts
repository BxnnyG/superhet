// Channel plans as one external store: the built-in plans, the custom ones from the server, the
// user's region and stepping preferences, and the lookup index derived from them. Plain TS so the
// strip, the palette and the panel share it (useSyncExternalStore).

import { builtinPlans } from "./catalog";
import { createChannelPlanApi, serverId, type ChannelPlanApi } from "./api";
import { buildIndex, type ChannelIndex } from "./lookup";
import { effectiveRegion, loadPrefs, savePrefs, type ChannelPrefs } from "./prefs";
import { browserRegion, regionMatches } from "./regions";
import type { Plan } from "./schema";

export interface ChannelSnapshot {
  prefs: ChannelPrefs;
  /** The region in force (explicit choice or the browser's). */
  region: string | null;
  /** Built-in and custom plans, the ones that apply in the region first. */
  plans: readonly Plan[];
  index: ChannelIndex;
  /** Custom plans only. */
  custom: readonly Plan[];
  /** Server documents this version could not read. */
  skipped: number;
  loaded: boolean;
  error: string | null;
}

const errorText = (e: unknown) => (e instanceof Error ? e.message : String(e));

/** Plans of the region first; otherwise the given order. */
export function orderPlans(plans: readonly Plan[], region: string | null): Plan[] {
  const inRegion = plans.filter((p) => regionMatches(p.regions, region));
  const other = plans.filter((p) => !regionMatches(p.regions, region));
  return [...inRegion, ...other];
}

export class ChannelStore {
  private snap: ChannelSnapshot;
  private readonly listeners = new Set<() => void>();
  private seq = 0;

  constructor(
    private readonly api: ChannelPlanApi = createChannelPlanApi(),
    prefs: ChannelPrefs = loadPrefs(),
    private readonly detected: string | null = browserRegion(),
    private readonly persist: (p: ChannelPrefs) => void = savePrefs,
  ) {
    this.snap = this.derive(prefs, [], 0, false, null);
  }

  subscribe = (fn: () => void): (() => void) => {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  };

  getSnapshot = (): ChannelSnapshot => this.snap;

  private derive(
    prefs: ChannelPrefs,
    custom: readonly Plan[],
    skipped: number,
    loaded: boolean,
    error: string | null,
  ): ChannelSnapshot {
    const region = effectiveRegion(prefs, this.detected);
    const plans = orderPlans([...builtinPlans(), ...custom], region);
    return { prefs, region, plans, index: buildIndex(plans), custom, skipped, loaded, error };
  }

  private set(
    patch: Partial<{
      prefs: ChannelPrefs;
      custom: readonly Plan[];
      skipped: number;
      loaded: boolean;
      error: string | null;
    }>,
  ): void {
    const s = this.snap;
    this.snap = this.derive(
      patch.prefs ?? s.prefs,
      patch.custom ?? s.custom,
      patch.skipped ?? s.skipped,
      patch.loaded ?? s.loaded,
      patch.error === undefined ? s.error : patch.error,
    );
    for (const fn of this.listeners) fn();
  }

  private setPrefs(prefs: ChannelPrefs): void {
    this.persist(prefs);
    this.set({ prefs });
  }

  /** Pick a country; null follows the browser locale again. */
  setRegion(region: string | null): void {
    this.setPrefs({ ...this.snap.prefs, region });
  }

  setStepByChannel(on: boolean): void {
    this.setPrefs({ ...this.snap.prefs, stepByChannel: on });
  }

  setRaster(planId: string, rasterId: string): void {
    const p = this.snap.prefs;
    this.setPrefs({ ...p, rasters: { ...p.rasters, [planId]: rasterId } });
  }

  /** Re-read the custom plans; only the newest read may land. */
  async refresh(): Promise<void> {
    const seq = ++this.seq;
    try {
      const { plans, skipped } = await this.api.list();
      if (seq === this.seq) this.set({ custom: plans, skipped, loaded: true, error: null });
    } catch (e) {
      if (seq === this.seq) this.set({ loaded: true, error: errorText(e) });
    }
  }

  /** Store a plan document on the server. Resolves with an error text or null. */
  async add(doc: Record<string, unknown>): Promise<string | null> {
    try {
      await this.api.create(doc);
    } catch (e) {
      return errorText(e);
    }
    await this.refresh();
    return null;
  }

  /** Delete a custom plan. Resolves with an error text or null. */
  async remove(plan: Plan): Promise<string | null> {
    const id = serverId(plan);
    if (id === null) return "built-in plans cannot be deleted";
    try {
      await this.api.remove(id);
    } catch (e) {
      await this.refresh();
      return errorText(e);
    }
    await this.refresh();
    return null;
  }
}
