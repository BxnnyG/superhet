// Channel plans on the spectrum overlay. The built-in services (PMR446, CB, DAB blocks, ...) are
// drawn from the overlay's own typed copy in bandplan/data.ts (plans.test.ts keeps both equal);
// custom plans imported by operators are added here as channel ticks in the "Imported" category,
// so the Display tab's band plan switch for it controls them.

import type { ChannelPlan as OverlayPlan } from "../bandplan/data";
import { MODE_LABEL } from "../vfo/model";
import { bandwidthOf, modeOf, type Plan } from "./schema";

/** Overlay channel lists for the plans (rasters are not drawn: thousands of ticks mean nothing). */
export function overlayPlans(plans: readonly Plan[]): OverlayPlan[] {
  return plans
    .filter((p) => p.channels.length > 0)
    .map((p) => {
      const mode = p.mode === "dab" ? undefined : p.mode;
      const bandwidthHz = p.bandwidthHz ?? undefined;
      return {
        kind: "list" as const,
        id: `x:${p.id}`,
        category: "custom" as const,
        name: p.name,
        ...(mode ? { mode } : {}),
        ...(mode ? { modeLabel: MODE_LABEL[mode] } : {}),
        ...(bandwidthHz ? { bandwidthHz } : {}),
        ...(p.note ? { note: p.note } : {}),
        source: p.source ?? "Custom channel plan",
        channels: p.channels.map((c) => {
          const m = modeOf(p, c);
          const bw = bandwidthOf(p, c);
          return {
            label: c.number,
            freqHz: c.freqHz,
            name: c.name ?? `${p.chip} ${c.number}`,
            ...(c.note ? { note: c.note } : {}),
            ...(m !== "dab" && m !== mode ? { mode: m } : {}),
            ...(bw !== null && bw !== bandwidthHz ? { bandwidthHz: bw } : {}),
          };
        }),
      };
    });
}
