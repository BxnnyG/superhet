// The map and table filters: altitude window, aircraft type, military, ground traffic.

import type { Tracked } from "./model";
import { groupOf, KIND_GROUPS, type KindGroupId, type SymbolKind } from "./symbols";

export type GroundFilter = "all" | "airborne" | "ground";

export interface AircraftFilter {
  /** Feet; null = no limit. Aircraft on the ground count as 0 ft. */
  altMinFt: number | null;
  altMaxFt: number | null;
  /** Type groups switched off (KIND_GROUPS). */
  hiddenGroups: readonly KindGroupId[];
  militaryOnly: boolean;
  ground: GroundFilter;
}

export const NO_FILTER: AircraftFilter = {
  altMinFt: null,
  altMaxFt: null,
  hiddenGroups: [],
  militaryOnly: false,
  ground: "all",
};

/** How many of the four filters narrow the picture (for the badge on the button). */
export function activeFilters(f: AircraftFilter): number {
  return (
    (f.altMinFt !== null || f.altMaxFt !== null ? 1 : 0) +
    (f.hiddenGroups.length > 0 ? 1 : 0) +
    (f.militaryOnly ? 1 : 0) +
    (f.ground !== "all" ? 1 : 0)
  );
}

/** What the filter needs to know about an aircraft besides its live data. */
export interface FilterFacts {
  kind: SymbolKind;
  military: boolean;
}

export function passes(a: Tracked, facts: FilterFacts, f: AircraftFilter): boolean {
  if (f.ground === "airborne" && a.onGround) return false;
  if (f.ground === "ground" && !a.onGround) return false;
  if (f.militaryOnly && !facts.military && facts.kind !== "fighter") return false;
  if (f.hiddenGroups.includes(groupOf(facts.kind))) return false;
  if (f.altMinFt !== null || f.altMaxFt !== null) {
    const ft = a.onGround ? 0 : a.altFt;
    // An aircraft that never reported an altitude cannot be said to be inside a window.
    if (ft === null) return false;
    if (f.altMinFt !== null && ft < f.altMinFt) return false;
    if (f.altMaxFt !== null && ft > f.altMaxFt) return false;
  }
  return true;
}

export function toggleGroup(f: AircraftFilter, id: KindGroupId): AircraftFilter {
  const hidden = f.hiddenGroups.includes(id)
    ? f.hiddenGroups.filter((g) => g !== id)
    : [...f.hiddenGroups, id];
  // Hiding every group would show nothing; the last switch stays on.
  return hidden.length >= KIND_GROUPS.length ? f : { ...f, hiddenGroups: hidden };
}
