// Band plan and bookmark overlay on the spectrum: a strip along the top edge with a thin ribbon of
// range bars (stacked in lanes where ranges overlap), channel ticks with short labels and
// clickable bookmark pins (a click applies the bookmark's whole receiver state).
// `layoutStrip` is pure (geometry in CSS px, relative to the plot's left edge); `AnnotationLayer`
// holds the data, draws the strip on the interaction canvas and hit-tests it. No React here.

import { builtinBandPlan, categoryColor, visibleChannels, visibleRanges } from "../bandplan/model";
import type { BandPlan, Channel } from "../bandplan/model";
import type { BandRange, CategoryId } from "../bandplan/data";
import {
  bookmarkColor,
  bookmarkTarget,
  digitalBadge,
  toneBadge,
  type Bookmark,
} from "../bookmarks/model";
import type { VfoMode } from "../vfo/api";
import { MODE_LABEL, formatBandwidth } from "../vfo/model";
import { formatFrequency } from "./freq";
import { CANVAS_SANS, CANVAS_TEXT, CANVAS_TEXT_STRONG, CHIP_BG, CHIP_BORDER } from "./palette";
import { spanWidth, type Span } from "./view";

export const TOP_PAD = 2;
export const LANE_H = 9;
export const MAX_LANES = 3;
/** Channel/bookmark lane: tick (and flag) on top, short label below. */
export const CHANNEL_LANE_H = 19;
const TICK_H = 7;
const STRIP_BOTTOM_PAD = 3;
const LABEL_PAD = 4;
/** Horizontal grab distance for ticks and flags (CSS px). */
const GRAB_PX = 5;

export interface AnnotationTarget {
  freqHz: number;
  mode?: VfoMode;
  bandwidthHz?: number;
  /** Squelch threshold in dBFS to apply. */
  squelchDbfs?: number;
  /** The squelch is meant to be set automatically (needs signal data, so it is not applied here). */
  squelchAuto?: boolean;
  /** Tuning step to select. */
  stepHz?: number;
  /** CTCSS tone squelch in Hz; null switches it off. Only set for NFM / AM capable targets. */
  toneSquelchHz?: number | null;
}

export interface BarItem {
  range: BandRange;
  lane: number;
  x0: number;
  x1: number;
  /** Text that fits inside the visible part of the bar, or null. */
  label: string | null;
}

export interface TickItem {
  channel: Channel;
  x: number;
  label: string | null;
}

export interface MarkItem {
  bookmark: Bookmark;
  x: number;
  label: string | null;
}

export interface StripLayout {
  lanes: number;
  /** Y (CSS px) where the channel lane starts; meaningful when `hasChannelLane`. */
  channelTop: number;
  hasChannelLane: boolean;
  /** Total height of the strip; everything else on the overlay starts below it. */
  insetPx: number;
  plotW: number;
  bars: BarItem[];
  ticks: TickItem[];
  marks: MarkItem[];
}

export const EMPTY_LAYOUT: StripLayout = {
  lanes: 0,
  channelTop: 0,
  hasChannelLane: false,
  insetPx: 0,
  plotW: 0,
  bars: [],
  ticks: [],
  marks: [],
};

export interface StripInput {
  plan: BandPlan;
  enabled: ReadonlySet<CategoryId>;
  bookmarks: readonly Bookmark[];
  span: Span;
  plotW: number;
  /** Text width in CSS px for the label font. */
  measure: (text: string) => number;
}

/** Assign ranges (sorted by start) to the first lane with room; the rest is dropped. */
export function assignLanes(
  ranges: readonly BandRange[],
  maxLanes = MAX_LANES,
): { range: BandRange; lane: number }[] {
  const laneEnd: number[] = [];
  const out: { range: BandRange; lane: number }[] = [];
  for (const range of ranges) {
    let lane = laneEnd.findIndex((end) => end <= range.startHz);
    if (lane < 0) lane = laneEnd.length;
    if (lane >= maxLanes) continue;
    laneEnd[lane] = range.endHz;
    out.push({ range, lane });
  }
  return out;
}

function overlaps(boxes: readonly [number, number][], a: number, b: number): boolean {
  return boxes.some(([x0, x1]) => a < x1 && b > x0);
}

export function layoutStrip(i: StripInput): StripLayout {
  const width = spanWidth(i.span);
  if (!(i.plotW > 0) || !(width > 0)) return EMPTY_LAYOUT;
  const hzPerPx = width / i.plotW;
  const xOf = (hz: number) => ((hz - i.span.startHz) / width) * i.plotW;

  const bars: BarItem[] = assignLanes(visibleRanges(i.plan, i.span, i.enabled, hzPerPx)).map(
    ({ range, lane }) => {
      const x0 = Math.max(0, xOf(range.startHz));
      const x1 = Math.min(i.plotW, xOf(range.endHz));
      const room = x1 - x0 - LABEL_PAD * 2;
      const label =
        [range.name, range.short].find((t) => t !== undefined && i.measure(t) <= room) ?? null;
      return { range, lane, x0, x1, label };
    },
  );
  const lanes = bars.reduce((n, b) => Math.max(n, b.lane + 1), 0);

  // Bookmark flags claim their label space first, channel labels fill what is left.
  const taken: [number, number][] = [];
  const marks: MarkItem[] = [];
  for (const bookmark of i.bookmarks) {
    if (bookmark.freqHz < i.span.startHz || bookmark.freqHz > i.span.endHz) continue;
    const x = xOf(bookmark.freqHz);
    const w = i.measure(bookmark.name);
    const fits = !overlaps(taken, x - 2, x + w + LABEL_PAD);
    if (fits) taken.push([x - 2, x + w + LABEL_PAD]);
    marks.push({ bookmark, x, label: fits ? bookmark.name : null });
  }
  marks.sort((a, b) => a.x - b.x);

  const ticks: TickItem[] = [];
  for (const { channel, detail } of visibleChannels(i.plan, i.span, i.enabled, hzPerPx)) {
    const x = xOf(channel.freqHz);
    let label: string | null = null;
    if (detail === "label") {
      const w = i.measure(channel.label);
      const box: [number, number] = [x - w / 2 - 1, x + w / 2 + 1];
      if (!overlaps(taken, box[0], box[1])) {
        taken.push(box);
        label = channel.label;
      }
    }
    ticks.push({ channel, x, label });
  }

  const hasChannelLane = ticks.length > 0 || marks.length > 0;
  const lanesH = lanes > 0 ? TOP_PAD + lanes * LANE_H : 0;
  const channelTop = lanesH + (lanes > 0 ? 1 : TOP_PAD);
  const insetPx = hasChannelLane
    ? channelTop + CHANNEL_LANE_H + STRIP_BOTTOM_PAD
    : lanes > 0
      ? lanesH + STRIP_BOTTOM_PAD
      : 0;
  return { lanes, channelTop, hasChannelLane, insetPx, plotW: i.plotW, bars, ticks, marks };
}

export type AnnotationHit =
  | { kind: "bookmark"; bookmark: Bookmark; x: number }
  | { kind: "channel"; channel: Channel; x: number }
  | { kind: "range"; range: BandRange; x: number };

/** What sits under (x, y) in the strip (plot-relative CSS px); bookmarks win over channels. */
export function hitStrip(layout: StripLayout, x: number, y: number): AnnotationHit | null {
  if (y < 0 || y >= layout.insetPx) return null;
  if (layout.hasChannelLane && y >= layout.channelTop) {
    let best: AnnotationHit | null = null;
    let bestDist = GRAB_PX + 0.001;
    for (const m of layout.marks) {
      const d = Math.abs(m.x - x);
      if (d < bestDist) {
        best = { kind: "bookmark", bookmark: m.bookmark, x: m.x };
        bestDist = d;
      }
    }
    if (best) return best;
    for (const t of layout.ticks) {
      const d = Math.abs(t.x - x);
      if (d < bestDist) {
        best = { kind: "channel", channel: t.channel, x: t.x };
        bestDist = d;
      }
    }
    if (best) return best;
  }
  const lane = Math.floor((y - TOP_PAD) / LANE_H);
  if (y >= TOP_PAD && lane >= 0 && lane < layout.lanes) {
    // Several bars can cover x in one lane only if they touch; take the narrowest.
    let best: BarItem | null = null;
    for (const b of layout.bars) {
      if (b.lane !== lane || x < b.x0 || x > b.x1) continue;
      if (!best || b.x1 - b.x0 < best.x1 - best.x0) best = b;
    }
    if (best) return { kind: "range", range: best.range, x };
  }
  return null;
}

/** The tuning a click on this hit performs, or null when it is not tunable. */
export function hitTarget(hit: AnnotationHit | null): AnnotationTarget | null {
  if (!hit) return null;
  if (hit.kind === "bookmark") return bookmarkTarget(hit.bookmark);
  if (hit.kind === "channel" && hit.channel.mode) {
    const c = hit.channel;
    return {
      freqHz: c.freqHz,
      mode: c.mode,
      ...(c.bandwidthHz ? { bandwidthHz: c.bandwidthHz } : {}),
    };
  }
  return null;
}

export interface Tooltip {
  title: string;
  color: string;
  lines: string[];
}

const mhz = (hz: number) => `${formatFrequency(hz)} MHz`;

export function hitTooltip(hit: AnnotationHit): Tooltip {
  const lines: string[] = [];
  if (hit.kind === "bookmark") {
    const b = hit.bookmark;
    const tech = [
      b.mode ? MODE_LABEL[b.mode] : null,
      b.bandwidthHz ? formatBandwidth(b.bandwidthHz) : null,
      toneBadge(b),
      digitalBadge(b),
    ]
      .filter(Boolean)
      .join(" · ");
    lines.push([mhz(b.freqHz), tech].filter(Boolean).join(" · "));
    if (b.tags.length > 0) lines.push(b.tags.join(", "));
    if (b.notes) lines.push(b.notes);
    lines.push("Bookmark · click to tune");
    return { title: b.name, color: bookmarkColor(b), lines };
  }
  if (hit.kind === "channel") {
    const c = hit.channel;
    const tech = [c.modeLabel, c.bandwidthHz ? formatBandwidth(c.bandwidthHz) : null]
      .filter(Boolean)
      .join(" · ");
    lines.push([mhz(c.freqHz), tech].filter(Boolean).join(" · "));
    if (c.note) lines.push(c.note);
    lines.push(
      `${c.approx ? "Approximate · " : ""}${c.mode ? "Click to tune" : "Not demodulable here"}`,
    );
    return { title: c.name, color: categoryColor(c.category), lines };
  }
  const r = hit.range;
  lines.push(`${mhz(r.startHz)} – ${mhz(r.endHz)}`);
  if (r.modes) lines.push(r.modes);
  if (r.note) lines.push(r.note);
  if (r.approx) lines.push("Approximate – check the current band plan");
  return { title: r.name, color: categoryColor(r.category), lines };
}

/** Wrap `text` to lines of at most `maxW` px using `measure` (greedy, by words). */
export function wrapText(text: string, maxW: number, measure: (t: string) => number): string[] {
  const out: string[] = [];
  let line = "";
  for (const word of text.split(/\s+/)) {
    const next = line === "" ? word : `${line} ${word}`;
    if (line !== "" && measure(next) > maxW) {
      out.push(line);
      line = word;
    } else {
      line = next;
    }
  }
  if (line !== "") out.push(line);
  return out;
}

export class AnnotationLayer {
  private plan: BandPlan = builtinBandPlan();
  private enabled: ReadonlySet<CategoryId> = new Set();
  private bookmarks: readonly Bookmark[] = [];
  private showBookmarks = false;
  private layout: StripLayout = EMPTY_LAYOUT;

  setPlan(plan: BandPlan): void {
    this.plan = plan;
  }

  setEnabled(enabled: ReadonlySet<CategoryId>): void {
    this.enabled = enabled;
  }

  setBookmarks(bookmarks: readonly Bookmark[], show: boolean): void {
    this.bookmarks = bookmarks;
    this.showBookmarks = show;
  }

  /** Height (CSS px) the strip took in the last layout; other overlays start below it. */
  get inset(): number {
    return this.layout.insetPx;
  }

  /** Recompute the strip for the span; call once per draw. */
  update(ctx: CanvasRenderingContext2D | null, span: Span, plotW: number, dpr: number): void {
    const measure = (text: string) => {
      if (!ctx) return text.length * 5.5;
      ctx.font = `${10 * dpr}px ${CANVAS_SANS}`;
      return ctx.measureText(text).width / dpr;
    };
    this.layout = layoutStrip({
      plan: this.plan,
      enabled: this.enabled,
      bookmarks: this.showBookmarks ? this.bookmarks : [],
      span,
      plotW,
      measure,
    });
  }

  hit(x: number, y: number): AnnotationHit | null {
    return hitStrip(this.layout, x, y);
  }

  /** Draw the strip; `left` is the plot's left edge in CSS px. */
  draw(ctx: CanvasRenderingContext2D, left: number, dpr: number, canvasH: number): void {
    const l = this.layout;
    if (l.insetPx === 0) return;
    ctx.save();
    ctx.beginPath();
    ctx.rect(left * dpr, 0, l.plotW * dpr, canvasH);
    ctx.clip();
    ctx.font = `${10 * dpr}px ${CANVAS_SANS}`;
    ctx.textBaseline = "middle";
    // Soft backing so labels stay readable over the trace.
    ctx.fillStyle = "rgba(9, 9, 11, 0.55)";
    ctx.fillRect(left * dpr, 0, l.plotW * dpr, l.insetPx * dpr);

    for (const b of l.bars) {
      const color = categoryColor(b.range.category);
      const y = (TOP_PAD + b.lane * LANE_H) * dpr;
      const x0 = (left + b.x0) * dpr;
      const w = Math.max(1, (b.x1 - b.x0) * dpr);
      ctx.globalAlpha = b.range.approx ? 0.55 : 1;
      ctx.fillStyle = color;
      ctx.fillRect(x0, y, w, 2 * dpr);
      ctx.globalAlpha = 0.12;
      ctx.fillRect(x0, y + 2 * dpr, w, (LANE_H - 3) * dpr);
      if (b.label) {
        ctx.globalAlpha = 1;
        ctx.fillStyle = CANVAS_TEXT_STRONG;
        ctx.textAlign = "center";
        ctx.fillText(b.label, x0 + w / 2, y + (LANE_H / 2 + 1) * dpr);
      }
    }
    ctx.globalAlpha = 1;

    const top = l.channelTop;
    for (const t of l.ticks) {
      const x = Math.round((left + t.x) * dpr) + 0.5 * dpr;
      ctx.strokeStyle = categoryColor(t.channel.category);
      ctx.globalAlpha = t.channel.mode ? 1 : 0.6;
      ctx.lineWidth = Math.max(1, dpr);
      ctx.beginPath();
      ctx.moveTo(x, top * dpr);
      ctx.lineTo(x, (top + TICK_H) * dpr);
      ctx.stroke();
      if (t.label) {
        ctx.globalAlpha = 1;
        ctx.fillStyle = CANVAS_TEXT;
        ctx.textAlign = "center";
        ctx.fillText(t.label, x, (top + TICK_H + 6) * dpr);
      }
    }
    ctx.globalAlpha = 1;

    for (const m of l.marks) {
      const color = bookmarkColor(m.bookmark);
      const x = (left + m.x) * dpr;
      ctx.fillStyle = color;
      ctx.strokeStyle = color;
      ctx.lineWidth = Math.max(1, dpr);
      // Pin: a down-pointing triangle (outlined when favourite) with a faint stem through the
      // rest of the canvas.
      ctx.beginPath();
      ctx.moveTo(x - 4 * dpr, top * dpr);
      ctx.lineTo(x + 4 * dpr, top * dpr);
      ctx.lineTo(x, (top + 7) * dpr);
      ctx.closePath();
      ctx.fill();
      if (m.bookmark.favourite) {
        ctx.strokeStyle = CANVAS_TEXT_STRONG;
        ctx.stroke();
        ctx.strokeStyle = color;
      }
      ctx.globalAlpha = 0.2;
      ctx.beginPath();
      ctx.moveTo(x, (top + 7) * dpr);
      ctx.lineTo(x, canvasH);
      ctx.stroke();
      ctx.globalAlpha = 1;
      if (m.label) {
        ctx.fillStyle = color;
        ctx.textAlign = "left";
        ctx.fillText(m.label, x + 5 * dpr, (top + TICK_H + 6) * dpr);
      }
    }
    ctx.restore();
  }

  /** Floating tooltip box for a hit, placed under the strip near x (CSS px, canvas coords). */
  drawTooltip(
    ctx: CanvasRenderingContext2D,
    hit: AnnotationHit,
    xCanvas: number,
    canvasW: number,
    dpr: number,
  ): void {
    const tip = hitTooltip(hit);
    const maxW = 260;
    const measure = (t: string) => {
      ctx.font = `${11 * dpr}px ${CANVAS_SANS}`;
      return ctx.measureText(t).width / dpr;
    };
    const body = tip.lines.flatMap((line) => wrapText(line, maxW, measure));
    ctx.font = `600 ${11 * dpr}px ${CANVAS_SANS}`;
    const titleW = ctx.measureText(tip.title).width / dpr;
    ctx.font = `${11 * dpr}px ${CANVAS_SANS}`;
    const textW = Math.max(titleW, ...body.map(measure), 0);
    const pad = 8;
    const lineH = 15;
    const w = Math.min(maxW + pad * 2, textW + pad * 2);
    const h = (body.length + 1) * lineH + pad * 2 - 4;
    const x = Math.min(Math.max(4, xCanvas - w / 2), canvasW / dpr - w - 4);
    const y = this.layout.insetPx + 6;
    ctx.save();
    ctx.beginPath();
    if (typeof ctx.roundRect === "function")
      ctx.roundRect(x * dpr, y * dpr, w * dpr, h * dpr, 6 * dpr);
    else ctx.rect(x * dpr, y * dpr, w * dpr, h * dpr);
    ctx.fillStyle = CHIP_BG;
    ctx.fill();
    ctx.lineWidth = Math.max(1, dpr);
    ctx.strokeStyle = CHIP_BORDER;
    ctx.stroke();
    ctx.textAlign = "left";
    ctx.textBaseline = "middle";
    ctx.fillStyle = tip.color;
    ctx.fillRect((x + 1) * dpr, (y + 6) * dpr, 2 * dpr, (h - 12) * dpr);
    ctx.font = `600 ${11 * dpr}px ${CANVAS_SANS}`;
    ctx.fillStyle = CANVAS_TEXT_STRONG;
    ctx.fillText(tip.title, (x + pad) * dpr, (y + pad + 3) * dpr);
    ctx.font = `${11 * dpr}px ${CANVAS_SANS}`;
    ctx.fillStyle = CANVAS_TEXT;
    body.forEach((line, n) => {
      ctx.fillText(line, (x + pad) * dpr, (y + pad + 3 + (n + 1) * lineH) * dpr);
    });
    ctx.restore();
  }
}
