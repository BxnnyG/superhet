// Panel preferences kept in localStorage: sort order and collapsed groups.

import type { SortKey } from "./model";

export const PREFS_KEY = "superhet.bookmarks.panel.v1";

export interface PanelPrefs {
  sort: SortKey;
  /** Group ids folded in the list; "none" is the ungrouped section. */
  collapsed: string[];
}

export const DEFAULT_PREFS: PanelPrefs = { sort: "freq", collapsed: [] };

export function loadPrefs(storage: Pick<Storage, "getItem"> = window.localStorage): PanelPrefs {
  try {
    const raw: unknown = JSON.parse(storage.getItem(PREFS_KEY) ?? "null");
    if (typeof raw !== "object" || raw === null) return DEFAULT_PREFS;
    const o = raw as Record<string, unknown>;
    return {
      sort: o.sort === "name" || o.sort === "recent" || o.sort === "freq" ? o.sort : "freq",
      collapsed: Array.isArray(o.collapsed)
        ? o.collapsed.filter((x): x is string => typeof x === "string")
        : [],
    };
  } catch {
    return DEFAULT_PREFS;
  }
}

export function savePrefs(
  prefs: PanelPrefs,
  storage: Pick<Storage, "setItem"> = window.localStorage,
): void {
  try {
    storage.setItem(PREFS_KEY, JSON.stringify(prefs));
  } catch {
    // Storage may be full or blocked; the preference is only a convenience.
  }
}
