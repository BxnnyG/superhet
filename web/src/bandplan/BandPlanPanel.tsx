import { useSyncExternalStore } from "react";
import { Toggle } from "../ui";
import { CATEGORIES } from "./data";
import { categoryOn, type OverlayStore } from "./settings";

interface Props {
  store: OverlayStore;
}

/** Sidebar section: master switch for the band plan overlay plus one switch per category. */
export function BandPlanPanel({ store }: Props) {
  const s = useSyncExternalStore(store.subscribe, store.getSnapshot);
  return (
    <div className="flex flex-col gap-3" role="group" aria-label="Band plan">
      <Toggle
        label="Show band plan"
        checked={s.bandPlan}
        onChange={(on) => store.set({ bandPlan: on })}
      />
      <ul className="flex flex-col gap-2" aria-label="Band plan categories">
        {CATEGORIES.filter((c) => c.id !== "custom").map((c) => (
          <li key={c.id} className="flex items-center justify-between gap-2">
            <Toggle
              label={c.label}
              checked={categoryOn(s, c.id)}
              disabled={!s.bandPlan}
              onChange={(on) => store.setCategory(c.id, on)}
            />
            <span
              aria-hidden="true"
              className="h-1 w-6 shrink-0 rounded-full"
              style={{ background: c.color }}
            />
          </li>
        ))}
      </ul>
      <p className="text-xs text-fg-subtle">
        Ranges and channels follow Germany / IARU Region 1. Hover for details, click a channel to
        tune the active VFO. Faded bars are approximate.
      </p>
    </div>
  );
}
