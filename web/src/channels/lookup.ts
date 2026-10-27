// Channel lookup and stepping over a set of plans. Pure: given the plans (built-in + custom), the
// user's region and raster choices, it answers "which channel is this frequency?" and "which
// channel comes next?" without touching the receiver.

import type { AnnotationTarget } from "../spectrum/annotations";
import { STEPS_HZ } from "../vfo/model";
import type { VfoMode } from "../vfo/api";
import { regionMatches } from "./regions";
import { bandwidthOf, modeOf, type Plan, type PlanChannel, type Raster } from "./schema";

/** Largest distance a frequency may have to a channel and still count as "on" it. */
export const MAX_TOLERANCE_HZ = 3_000;
/** Smallest tolerance, so close channel grids still accept a slightly detuned VFO. */
export const MIN_TOLERANCE_HZ = 500;

/** A quarter of the channel spacing, between 500 Hz and 3 kHz. */
export function toleranceFor(spacingHz: number): number {
  if (!Number.isFinite(spacingHz)) return MAX_TOLERANCE_HZ;
  return Math.min(MAX_TOLERANCE_HZ, Math.max(MIN_TOLERANCE_HZ, spacingHz / 4));
}

export interface ChannelMatch {
  plan: Plan;
  channel: PlanChannel;
  /** Position in `plan.channels`, or in the raster. */
  index: number;
  /** Set when the channel comes from a raster rather than the list. */
  raster: Raster | null;
  /** Signed distance of the frequency to the channel, Hz (frequency - channel). */
  offsetHz: number;
}

// ---------------------------------------------------------------------------------------------
// Rasters

/** Number of channels in the raster. */
export function rasterCount(r: Raster): number {
  return Math.floor((r.endHz - r.startHz) / r.stepHz + 1e-6) + 1;
}

export function rasterFreq(r: Raster, i: number): number {
  return Math.round(r.startHz + i * r.stepHz);
}

const ICAO_833 = [5, 10, 15] as const;

/**
 * The name of raster channel `i`: the frequency in MHz with three decimals, or for `icao-8.33`
 * the pilot's channel designator. Three 8.33 kHz channels share one 25 kHz block and are named
 * after +5, +10 and +15 kHz of it: 118.005, 118.010, 118.015, then 118.030, 118.035, 118.040.
 */
export function rasterName(r: Raster, i: number): string {
  if (r.naming === "icao-8.33") {
    const offsetKhz = Math.floor(i / 3) * 25 + (ICAO_833[i % 3] ?? 0);
    return ((r.startHz / 1000 + offsetKhz) / 1000).toFixed(3);
  }
  return (rasterFreq(r, i) / 1e6).toFixed(3);
}

export function rasterChannel(r: Raster, i: number): PlanChannel {
  return {
    number: rasterName(r, i),
    name: null,
    freqHz: rasterFreq(r, i),
    ctcssHz: null,
    note: null,
    mode: null,
    bandwidthHz: null,
  };
}

/** The raster the user picked for a plan (default: the first). */
export function activeRaster(plan: Plan, choice: Readonly<Record<string, string>>): Raster | null {
  return plan.rasters.find((r) => r.id === choice[plan.id]) ?? plan.rasters[0] ?? null;
}

// ---------------------------------------------------------------------------------------------
// Index

interface Entry {
  plan: Plan;
  channel: PlanChannel;
  index: number;
  tolHz: number;
  order: number;
}

export interface ChannelIndex {
  plans: readonly Plan[];
  /** Listed channels sorted by frequency (DAB blocks excluded: no VFO sits on them). */
  entries: readonly Entry[];
}

/** Smallest distance between two different channel frequencies of the plan. */
export function planSpacing(plan: Plan): number {
  const f = [...new Set(plan.channels.map((c) => c.freqHz))].sort((a, b) => a - b);
  let spacing = Infinity;
  for (let i = 1; i < f.length; i++) spacing = Math.min(spacing, (f[i] ?? 0) - (f[i - 1] ?? 0));
  return spacing;
}

export function buildIndex(plans: readonly Plan[]): ChannelIndex {
  const entries: Entry[] = [];
  plans.forEach((plan, order) => {
    const tolHz = toleranceFor(planSpacing(plan));
    plan.channels.forEach((channel, index) => {
      if (modeOf(plan, channel) === "dab") return;
      entries.push({ plan, channel, index, tolHz, order });
    });
  });
  entries.sort((a, b) => a.channel.freqHz - b.channel.freqHz);
  return { plans, entries };
}

function lowerBound(entries: readonly Entry[], hz: number): number {
  let lo = 0;
  let hi = entries.length;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if ((entries[mid]?.channel.freqHz ?? Infinity) < hz) lo = mid + 1;
    else hi = mid;
  }
  return lo;
}

export interface MatchOptions {
  /** The user's region; plans that apply there win ties. */
  region: string | null;
  /** Raster choice per plan id. */
  rasters: Readonly<Record<string, string>>;
}

interface Candidate extends ChannelMatch {
  order: number;
}

/** The channel `hz` sits on: nearest first, then plans of the user's region, then plan order. */
export function matchChannel(
  index: ChannelIndex,
  hz: number,
  opts: MatchOptions,
): ChannelMatch | null {
  const found: Candidate[] = [];
  for (let i = lowerBound(index.entries, hz - MAX_TOLERANCE_HZ); i < index.entries.length; i++) {
    const e = index.entries[i];
    if (!e || e.channel.freqHz > hz + MAX_TOLERANCE_HZ) break;
    const offsetHz = hz - e.channel.freqHz;
    if (Math.abs(offsetHz) > e.tolHz) continue;
    found.push({
      plan: e.plan,
      channel: e.channel,
      index: e.index,
      raster: null,
      offsetHz,
      order: e.order,
    });
  }
  index.plans.forEach((plan, order) => {
    const raster = activeRaster(plan, opts.rasters);
    if (raster === null) return;
    const tol = toleranceFor(raster.stepHz);
    if (hz < raster.startHz - tol || hz > raster.endHz + tol) return;
    const n = rasterCount(raster);
    const i = Math.min(n - 1, Math.max(0, Math.round((hz - raster.startHz) / raster.stepHz)));
    const offsetHz = hz - rasterFreq(raster, i);
    if (Math.abs(offsetHz) > tol) return;
    found.push({ plan, channel: rasterChannel(raster, i), index: i, raster, offsetHz, order });
  });
  const rank = (c: Candidate) => [
    Math.abs(c.offsetHz),
    regionMatches(c.plan.regions, opts.region) ? 0 : 1,
    c.order,
  ];
  let best: Candidate | undefined;
  for (const c of found) {
    if (best === undefined) {
      best = c;
      continue;
    }
    const a = rank(c);
    const b = rank(best);
    const cmp = (a[0] ?? 0) - (b[0] ?? 0) || (a[1] ?? 0) - (b[1] ?? 0) || (a[2] ?? 0) - (b[2] ?? 0);
    if (cmp < 0) best = c;
  }
  if (best === undefined) return null;
  return {
    plan: best.plan,
    channel: best.channel,
    index: best.index,
    raster: best.raster,
    offsetHz: best.offsetHz,
  };
}

/**
 * The neighbouring channel of a match in plan order (`dir` +1 up, -1 down), or null at the end of
 * the plan. List plans step in channel-list order (CB 22 -> 23 -> 24 even where the frequencies
 * are not monotonic); rasters step along the grid.
 */
export function stepChannel(match: ChannelMatch, dir: 1 | -1): ChannelMatch | null {
  const j = match.index + dir;
  if (match.raster) {
    if (j < 0 || j >= rasterCount(match.raster)) return null;
    return {
      ...match,
      channel: rasterChannel(match.raster, j),
      index: j,
      offsetHz: 0,
    };
  }
  // Skip DAB blocks inside a mixed plan: a VFO cannot sit there.
  for (let k = j; k >= 0 && k < match.plan.channels.length; k += dir) {
    const channel = match.plan.channels[k];
    if (channel && modeOf(match.plan, channel) !== "dab") {
      return { ...match, channel, index: k, offsetHz: 0 };
    }
  }
  return null;
}

/** Step `steps` channels (negative = down); stops at the end of the plan. Null if it cannot move. */
export function stepChannelBy(match: ChannelMatch, steps: number): ChannelMatch | null {
  const dir = steps < 0 ? -1 : 1;
  let current: ChannelMatch | null = null;
  let from = match;
  for (let i = 0; i < Math.abs(steps); i++) {
    const next = stepChannel(from, dir);
    if (next === null) break;
    current = next;
    from = next;
  }
  return current;
}

/** "PMR 3", "Marine 16", "Air 118.005". */
export function chipLabel(plan: Plan, channel: PlanChannel): string {
  return `${plan.chip} ${channel.number}`;
}

/** Title of a channel: its name when it has one, else the chip label. */
export function channelTitle(plan: Plan, channel: PlanChannel): string {
  const label = chipLabel(plan, channel);
  return channel.name ? `${label} · ${channel.name}` : label;
}

/** The VFO settings a channel tunes to; null for channels without a VFO mode (DAB). */
export function channelTarget(plan: Plan, channel: PlanChannel): AnnotationTarget | null {
  const mode = modeOf(plan, channel);
  if (mode === "dab") return null;
  const vfoMode: VfoMode = mode;
  const bandwidthHz = bandwidthOf(plan, channel);
  const stepHz = plan.stepHz;
  return {
    freqHz: channel.freqHz,
    mode: vfoMode,
    ...(bandwidthHz !== null ? { bandwidthHz } : {}),
    ...(channel.ctcssHz !== null && (vfoMode === "nfm" || vfoMode === "am")
      ? { toneSquelchHz: channel.ctcssHz }
      : {}),
    ...(stepHz !== null && (STEPS_HZ as readonly number[]).includes(stepHz) ? { stepHz } : {}),
  };
}
