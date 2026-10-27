// Per-browser channel settings, persisted in localStorage.

import { browserRegion } from "./regions";

export const CHANNEL_PREFS_KEY = "superhet.channels.v1";

export interface ChannelPrefs {
  /** The user's choice of country code; null = follow the browser locale. */
  region: string | null;
  /** The < > buttons step channels while the VFO sits on one (otherwise always by kHz). */
  stepByChannel: boolean;
  /** Raster picked per plan id (the air band's 25 kHz or 8.33 kHz grid). */
  rasters: Record<string, string>;
}

export const DEFAULT_PREFS: ChannelPrefs = { region: null, stepByChannel: true, rasters: {} };

export function loadPrefs(storage: Pick<Storage, "getItem"> = window.localStorage): ChannelPrefs {
  try {
    const raw: unknown = JSON.parse(storage.getItem(CHANNEL_PREFS_KEY) ?? "null");
    if (typeof raw !== "object" || raw === null) return DEFAULT_PREFS;
    const o = raw as Record<string, unknown>;
    const rasters: Record<string, string> = {};
    if (typeof o.rasters === "object" && o.rasters !== null) {
      for (const [k, v] of Object.entries(o.rasters)) if (typeof v === "string") rasters[k] = v;
    }
    return {
      region: typeof o.region === "string" && /^[A-Z]{2}$/.test(o.region) ? o.region : null,
      stepByChannel: typeof o.stepByChannel === "boolean" ? o.stepByChannel : true,
      rasters,
    };
  } catch {
    return DEFAULT_PREFS;
  }
}

export function savePrefs(
  prefs: ChannelPrefs,
  storage: Pick<Storage, "setItem"> = window.localStorage,
): void {
  try {
    storage.setItem(CHANNEL_PREFS_KEY, JSON.stringify(prefs));
  } catch {
    // Not persisted; the choice still applies for this session.
  }
}

/** The region in force: the explicit choice, else the browser's, else none. */
export function effectiveRegion(
  prefs: ChannelPrefs,
  detected: string | null = browserRegion(),
): string | null {
  return prefs.region ?? detected;
}
