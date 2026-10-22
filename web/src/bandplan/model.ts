// Pure band plan logic: expanding channel plans into channels, what is visible at a zoom level,
// matching a frequency to its band plan entry and the JSON import hook for further sources.

import type { VfoMode } from "../vfo/api";
import { VFO_MODES } from "../vfo/api";
import { MODE_LABEL } from "../vfo/model";
import {
  CATEGORIES,
  CATEGORY_BY_ID,
  CHANNEL_PLANS,
  RANGES,
  type BandRange,
  type CategoryId,
  type ChannelPlan,
} from "./data";

export interface Channel {
  /** Unique within a band plan. */
  key: string;
  planId: string;
  category: CategoryId;
  /** Short text next to the tick ("12", "5A"). */
  label: string;
  /** Full name for tooltips and bookmark suggestions. */
  name: string;
  freqHz: number;
  /** VFO mode a click tunes with; undefined = not demodulable here, click does nothing. */
  mode?: VfoMode;
  /** Modulation in words. */
  modeLabel?: string;
  bandwidthHz?: number;
  note?: string;
  approx: boolean;
  source: string;
  /** Smallest channel distance of the plan (Infinity for a single channel). */
  spacingHz: number;
}

export interface BandPlan {
  /** Sorted by start frequency, wider first on ties. */
  ranges: readonly BandRange[];
  /** Sorted by frequency. */
  channels: readonly Channel[];
}

export function expandPlan(plan: ChannelPlan): Channel[] {
  const base = {
    planId: plan.id,
    category: plan.category,
    approx: plan.approx === true,
    source: plan.source,
  };
  const make = (
    label: string,
    freqHz: number,
    own: { name?: string; note?: string; mode?: VfoMode; bandwidthHz?: number } = {},
  ): Omit<Channel, "spacingHz"> => {
    const mode = own.mode ?? plan.mode;
    const note = own.note ?? plan.note;
    const modeLabel = plan.modeLabel ?? (mode ? MODE_LABEL[mode] : undefined);
    const bandwidthHz = own.bandwidthHz ?? plan.bandwidthHz;
    return {
      ...base,
      key: `${plan.id}:${freqHz}`,
      label,
      name: own.name ?? `${plan.name} ${label}`.trim(),
      freqHz,
      ...(mode ? { mode } : {}),
      ...(modeLabel ? { modeLabel } : {}),
      ...(bandwidthHz ? { bandwidthHz } : {}),
      ...(note ? { note } : {}),
    };
  };
  const raw: Omit<Channel, "spacingHz">[] =
    plan.kind === "grid"
      ? Array.from({ length: plan.count }, (_, i) =>
          make(String((plan.firstNumber ?? 1) + i), plan.firstHz + i * plan.stepHz),
        )
      : plan.channels.map((c) => make(c.label, c.freqHz, c));
  raw.sort((a, b) => a.freqHz - b.freqHz);
  let spacing = plan.kind === "grid" ? plan.stepHz : Infinity;
  if (plan.kind === "list") {
    for (let i = 1; i < raw.length; i++) {
      const d = (raw[i]?.freqHz ?? 0) - (raw[i - 1]?.freqHz ?? 0);
      if (d > 0) spacing = Math.min(spacing, d);
    }
  }
  return raw.map((c) => ({ ...c, spacingHz: spacing }));
}

const byStart = (a: BandRange, b: BandRange) =>
  a.startHz - b.startHz || b.endHz - b.startHz - (a.endHz - a.startHz);

/** Further band plan data in the JSON shape documented in docs/bandplan.md. */
export interface ExtraBandPlan {
  ranges: BandRange[];
  plans: ChannelPlan[];
}

export function buildBandPlan(extra?: ExtraBandPlan): BandPlan {
  const ranges = [...RANGES, ...(extra?.ranges ?? [])].sort(byStart);
  const channels = [...CHANNEL_PLANS, ...(extra?.plans ?? [])]
    .flatMap(expandPlan)
    .sort((a, b) => a.freqHz - b.freqHz);
  return { ranges, channels };
}

let builtin: BandPlan | null = null;
export function builtinBandPlan(): BandPlan {
  builtin ??= buildBandPlan();
  return builtin;
}

// ---------------------------------------------------------------------------------------------
// Visibility

/** Channel ticks need at least this many CSS px between neighbours, else they would be a comb. */
export const MIN_TICK_PX = 4;
/** Channel labels need this much room per channel. */
export const MIN_LABEL_PX = 22;
/** Range bars narrower than this are not drawn. */
export const MIN_BAR_PX = 1;

export type ChannelDetail = "hidden" | "tick" | "label";

/** How much of a plan's channels to draw at `hzPerPx` (isolated channels always get a label). */
export function channelDetail(spacingHz: number, hzPerPx: number): ChannelDetail {
  if (!(hzPerPx > 0)) return "hidden";
  const px = spacingHz / hzPerPx;
  return px >= MIN_LABEL_PX ? "label" : px >= MIN_TICK_PX ? "tick" : "hidden";
}

export interface Span {
  startHz: number;
  endHz: number;
}

export function visibleRanges(
  plan: BandPlan,
  span: Span,
  enabled: ReadonlySet<CategoryId>,
  hzPerPx: number,
): BandRange[] {
  return plan.ranges.filter(
    (r) =>
      enabled.has(r.category) &&
      r.endHz > span.startHz &&
      r.startHz < span.endHz &&
      (r.endHz - r.startHz) / hzPerPx >= MIN_BAR_PX,
  );
}

/** First index in `channels` (sorted) with freqHz >= hz. */
function lowerBound(channels: readonly Channel[], hz: number): number {
  let lo = 0;
  let hi = channels.length;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if ((channels[mid]?.freqHz ?? Infinity) < hz) lo = mid + 1;
    else hi = mid;
  }
  return lo;
}

export interface VisibleChannel {
  channel: Channel;
  detail: "tick" | "label";
}

export function visibleChannels(
  plan: BandPlan,
  span: Span,
  enabled: ReadonlySet<CategoryId>,
  hzPerPx: number,
): VisibleChannel[] {
  const out: VisibleChannel[] = [];
  for (let i = lowerBound(plan.channels, span.startHz); i < plan.channels.length; i++) {
    const c = plan.channels[i];
    if (!c || c.freqHz > span.endHz) break;
    if (!enabled.has(c.category)) continue;
    const detail = channelDetail(c.spacingHz, hzPerPx);
    if (detail !== "hidden") out.push({ channel: c, detail });
  }
  return out;
}

// ---------------------------------------------------------------------------------------------
// Matching

export interface BandMatch {
  channel: Channel | null;
  range: BandRange | null;
}

/** Largest distance (Hz) at which a frequency still counts as "on" a channel. */
export function channelTolerance(c: Channel): number {
  return Math.max(1_000, Math.min(c.spacingHz / 2, 6_250));
}

/** The channel the frequency sits on and the narrowest range containing it. */
export function matchBandPlan(
  plan: BandPlan,
  hz: number,
  enabled?: ReadonlySet<CategoryId>,
): BandMatch {
  let channel: Channel | null = null;
  let best = Infinity;
  const reach = 30_000;
  for (let i = lowerBound(plan.channels, hz - reach); i < plan.channels.length; i++) {
    const c = plan.channels[i];
    if (!c || c.freqHz > hz + reach) break;
    if (enabled && !enabled.has(c.category)) continue;
    const d = Math.abs(c.freqHz - hz);
    if (d <= channelTolerance(c) && d < best) {
      channel = c;
      best = d;
    }
  }
  let range: BandRange | null = null;
  for (const r of plan.ranges) {
    if (r.startHz > hz) break;
    if (hz > r.endHz || (enabled && !enabled.has(r.category))) continue;
    if (!range || r.endHz - r.startHz < range.endHz - range.startHz) range = r;
  }
  return { channel, range };
}

/** Name proposal for a bookmark at `hz`, from the band plan; "" when nothing matches. */
export function suggestName(match: BandMatch): string {
  if (match.channel) return match.channel.name;
  return match.range ? match.range.name : "";
}

// ---------------------------------------------------------------------------------------------
// Import hook

const CATEGORY_IDS = new Set<string>(CATEGORIES.map((c) => c.id));

type Rec = Record<string, unknown>;
const isRec = (v: unknown): v is Rec => typeof v === "object" && v !== null && !Array.isArray(v);
const posNum = (v: unknown): number | null =>
  typeof v === "number" && Number.isFinite(v) && v > 0 ? v : null;
const optStr = (v: unknown): string | undefined =>
  typeof v === "string" && v.trim() !== "" ? v : undefined;

function category(v: unknown): CategoryId {
  return typeof v === "string" && CATEGORY_IDS.has(v) ? (v as CategoryId) : "custom";
}

/**
 * Validate a JSON document from another source (see docs/bandplan.md) and turn it into band plan
 * data. Unknown categories become "custom"; invalid entries are skipped. Throws on a document
 * that is not a version 1 band plan at all.
 */
export function parseExtraBandPlan(raw: unknown, sourceName = "imported"): ExtraBandPlan {
  if (!isRec(raw) || raw.v !== 1) throw new Error("not a version 1 band plan document");
  const source = optStr(raw.name) ?? sourceName;
  const ranges: BandRange[] = [];
  if (Array.isArray(raw.ranges)) {
    raw.ranges.forEach((r: unknown, i) => {
      if (!isRec(r)) return;
      const startHz = posNum(r.start_hz);
      const endHz = posNum(r.end_hz);
      const name = optStr(r.name);
      if (startHz === null || endHz === null || endHz <= startHz || !name) return;
      const modes = optStr(r.modes);
      const note = optStr(r.note);
      ranges.push({
        id: `x:${source}:${i}`,
        category: category(r.category),
        name,
        startHz,
        endHz,
        ...(modes ? { modes } : {}),
        ...(note ? { note } : {}),
        ...(r.approx === true ? { approx: true } : {}),
        source: optStr(r.source) ?? source,
      });
    });
  }
  const plans: ChannelPlan[] = [];
  if (Array.isArray(raw.channels)) {
    raw.channels.forEach((c: unknown, i) => {
      if (!isRec(c)) return;
      const freqHz = posNum(c.freq_hz);
      const name = optStr(c.name);
      if (freqHz === null || !name) return;
      const mode = VFO_MODES.find((m) => m === c.mode);
      const bandwidthHz = posNum(c.bandwidth_hz);
      const note = optStr(c.note);
      plans.push({
        kind: "list",
        id: `x:${source}:ch${i}`,
        category: category(c.category),
        name,
        ...(mode ? { mode } : {}),
        ...(bandwidthHz ? { bandwidthHz } : {}),
        ...(note ? { note } : {}),
        ...(c.approx === true ? { approx: true } : {}),
        source: optStr(c.source) ?? source,
        channels: [{ label: optStr(c.label) ?? name, freqHz, name }],
      });
    });
  }
  return { ranges, plans };
}

export function categoryColor(id: CategoryId): string {
  return CATEGORY_BY_ID.get(id)?.color ?? "#a1a1aa";
}
