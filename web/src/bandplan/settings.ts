// Which band plan categories and bookmarks are drawn on the spectrum; persisted in localStorage.

import { CATEGORIES, type CategoryId } from "./data";

export const BANDPLAN_KEY = "superhet.bandplan.v1";

export interface OverlaySettings {
  /** Master switch for the band plan (ranges and channel markers). */
  bandPlan: boolean;
  /** Per category; categories missing here use their default. */
  categories: Partial<Record<CategoryId, boolean>>;
  /** Draw the user's bookmarks on the spectrum. */
  bookmarks: boolean;
}

export const DEFAULT_OVERLAY: OverlaySettings = { bandPlan: true, categories: {}, bookmarks: true };

export function loadOverlay(
  storage: Pick<Storage, "getItem"> = window.localStorage,
): OverlaySettings {
  try {
    const raw: unknown = JSON.parse(storage.getItem(BANDPLAN_KEY) ?? "null");
    if (typeof raw !== "object" || raw === null) return DEFAULT_OVERLAY;
    const o = raw as Record<string, unknown>;
    const categories: Partial<Record<CategoryId, boolean>> = {};
    if (typeof o.categories === "object" && o.categories !== null) {
      const c = o.categories as Record<string, unknown>;
      for (const cat of CATEGORIES) {
        if (typeof c[cat.id] === "boolean") categories[cat.id] = c[cat.id] as boolean;
      }
    }
    return {
      bandPlan: typeof o.bandPlan === "boolean" ? o.bandPlan : true,
      categories,
      bookmarks: typeof o.bookmarks === "boolean" ? o.bookmarks : true,
    };
  } catch {
    return DEFAULT_OVERLAY;
  }
}

export function categoryOn(s: OverlaySettings, id: CategoryId): boolean {
  return s.categories[id] ?? CATEGORIES.find((c) => c.id === id)?.defaultOn ?? false;
}

/** The categories drawn right now (empty when the master switch is off). */
export function enabledCategories(s: OverlaySettings): ReadonlySet<CategoryId> {
  return new Set(s.bandPlan ? CATEGORIES.filter((c) => categoryOn(s, c.id)).map((c) => c.id) : []);
}

/** Small external store so the spectrum overlay and the sidebar share the settings. */
export class OverlayStore {
  private state: OverlaySettings;
  private readonly listeners = new Set<() => void>();

  constructor(
    initial: OverlaySettings = loadOverlay(),
    private readonly storage: Pick<Storage, "setItem"> | null = window.localStorage,
  ) {
    this.state = initial;
  }

  subscribe = (fn: () => void): (() => void) => {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  };

  getSnapshot = (): OverlaySettings => this.state;

  set(patch: Partial<Omit<OverlaySettings, "categories">>): void {
    this.apply({ ...this.state, ...patch });
  }

  setCategory(id: CategoryId, on: boolean): void {
    this.apply({ ...this.state, categories: { ...this.state.categories, [id]: on } });
  }

  private apply(next: OverlaySettings): void {
    this.state = next;
    try {
      this.storage?.setItem(BANDPLAN_KEY, JSON.stringify(next));
    } catch {
      // Not persisted.
    }
    for (const fn of this.listeners) fn();
  }
}
