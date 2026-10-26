// Frequencies recently tuned from the command palette (typed or picked), kept in this browser.

import type { VfoMode } from "../vfo/api";
import { isVfoMode } from "../vfo/api";

export interface RecentEntry {
  freqHz: number;
  mode?: VfoMode;
  toneHz?: number;
}

export const RECENT_KEY = "superhet.palette.recent.v1";
export const RECENT_MAX = 8;

const same = (a: RecentEntry, b: RecentEntry) =>
  a.freqHz === b.freqHz && a.mode === b.mode && a.toneHz === b.toneHz;

/** The entry first, any earlier copy of it dropped, capped at RECENT_MAX. */
export function pushRecent(list: readonly RecentEntry[], entry: RecentEntry): RecentEntry[] {
  return [entry, ...list.filter((e) => !same(e, entry))].slice(0, RECENT_MAX);
}

function parse(raw: unknown): RecentEntry | null {
  if (typeof raw !== "object" || raw === null) return null;
  const r = raw as Record<string, unknown>;
  if (typeof r.freqHz !== "number" || !Number.isFinite(r.freqHz) || r.freqHz <= 0) return null;
  return {
    freqHz: r.freqHz,
    ...(isVfoMode(r.mode) ? { mode: r.mode } : {}),
    ...(typeof r.toneHz === "number" && Number.isFinite(r.toneHz) ? { toneHz: r.toneHz } : {}),
  };
}

export function loadRecent(storage: Pick<Storage, "getItem"> = window.localStorage): RecentEntry[] {
  try {
    const raw: unknown = JSON.parse(storage.getItem(RECENT_KEY) ?? "[]");
    if (!Array.isArray(raw)) return [];
    return raw
      .map(parse)
      .filter((e): e is RecentEntry => e !== null)
      .slice(0, RECENT_MAX);
  } catch {
    return [];
  }
}

export function saveRecent(
  list: readonly RecentEntry[],
  storage: Pick<Storage, "setItem"> = window.localStorage,
): void {
  try {
    storage.setItem(RECENT_KEY, JSON.stringify(list));
  } catch {
    // Not persisted; the list still works for this session.
  }
}
