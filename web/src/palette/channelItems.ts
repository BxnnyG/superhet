// Palette entries for channel words ("pmr 3", "dab 5c", "ch16"): resolved through the channel
// plans, one entry per matching channel. Kept apart from items.ts so the palette core stays small.

import { channelTitle } from "../channels/lookup";
import { resolveChannelQuery } from "../channels/query";
import { modeOf } from "../channels/schema";
import { formatFrequency } from "../spectrum/freq";
import type { ChannelPaletteDeps, PaletteItem } from "./items";

/** Most channel entries shown for one query. */
const MAX_HITS = 6;

export function channelItems(deps: ChannelPaletteDeps, query: string): PaletteItem[] {
  return resolveChannelQuery(deps.plans, query, deps.region)
    .slice(0, MAX_HITS)
    .map(({ plan, channel }) => {
      const dab = modeOf(plan, channel) === "dab";
      return {
        id: `channel:${plan.id}:${channel.number}`,
        group: "Channels" as const,
        title: `${channelTitle(plan, channel)} · ${formatFrequency(channel.freqHz)} MHz`,
        subtitle: dab ? `${plan.name} · moves the device centre` : plan.name,
        keywords: [plan.name, plan.service, channel.number],
        newVfoAlt: !dab,
        run: ({ newVfo }) => deps.tuneChannel(plan, channel, newVfo),
      };
    });
}
