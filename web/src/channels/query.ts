// Channel words for the command palette: "pmr 3", "freenet2", "dab 5c", "marine 16", "ch16".
// A word that names a plan (alias, id or chip) followed by a channel number resolves through the
// plans; pure, so it is unit-tested without the palette.

import { regionMatches } from "./regions";
import { normalizeNumber, type Plan, type PlanChannel } from "./schema";

export interface ChannelHit {
  plan: Plan;
  channel: PlanChannel;
}

/** All words a plan answers to. */
export function planWords(plan: Plan): string[] {
  return [
    ...new Set([
      ...plan.aliases,
      plan.id.toLowerCase(),
      plan.chip.toLowerCase().replace(/\s+/g, ""),
    ]),
  ];
}

/** Split "pmr 3" into [["pmr","3"]] and "pmr3" into every word/number split that is possible. */
function candidates(input: string): [word: string, number: string][] {
  const tokens = input.trim().toLowerCase().split(/\s+/).filter(Boolean);
  if (tokens.length === 0) return [];
  if (tokens.length > 1) {
    const [word = "", ...rest] = tokens;
    return [[word, rest.join("")]];
  }
  const t = tokens[0] ?? "";
  const out: [string, string][] = [];
  for (let i = 1; i < t.length; i++) out.push([t.slice(0, i), t.slice(i)]);
  return out;
}

/**
 * Channels the query names. Plans that apply in `region` come first; a number can exist in
 * several plans (CB 45 only in the German extension, "ch" only means marine).
 */
export function resolveChannelQuery(
  plans: readonly Plan[],
  input: string,
  region: string | null,
): ChannelHit[] {
  const hits: ChannelHit[] = [];
  const seen = new Set<string>();
  for (const [word, rawNumber] of candidates(input)) {
    const number = normalizeNumber(rawNumber.replace(/^#/, ""));
    if (number === "") continue;
    for (const plan of plans) {
      if (!planWords(plan).includes(word)) continue;
      for (const channel of plan.channels) {
        if (normalizeNumber(channel.number) !== number) continue;
        const key = `${plan.id}\u0000${channel.number}`;
        if (seen.has(key)) continue;
        seen.add(key);
        hits.push({ plan, channel });
      }
    }
  }
  return hits.sort(
    (a, b) =>
      Number(!regionMatches(a.plan.regions, region)) -
      Number(!regionMatches(b.plan.regions, region)),
  );
}
