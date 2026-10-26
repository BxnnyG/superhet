// Bookmark types and pure helpers (filtering, sorting, grouping, tag handling, de-duplication).
// A bookmark is a point channel with the receiver state needed to listen to it; band plan ranges
// (start/end) live in ../bandplan and never appear here.

import { CTCSS_TONES } from "../audio/tones";
import type { AnnotationTarget } from "../spectrum/annotations";
import type { Vfo, VfoMode } from "../vfo/api";
import type { StepChoice } from "../vfo/model";

export type ToneMode = "none" | "ctcss" | "dcs";
export type DcsPolarity = "N" | "R";
export type DigitalProtocol = "tetra" | "dmr" | "dpmr" | "p25";

export const DIGITAL_PROTOCOLS: readonly DigitalProtocol[] = ["tetra", "dmr", "dpmr", "p25"];
export const PROTOCOL_LABEL: Record<DigitalProtocol, string> = {
  tetra: "TETRA",
  dmr: "DMR",
  dpmr: "dPMR",
  p25: "P25",
};
/** Highest timeslot number per protocol (0 = the protocol has no timeslots in the model). */
export const MAX_TIMESLOT: Record<DigitalProtocol, number> = { tetra: 4, dmr: 2, dpmr: 0, p25: 0 };

export interface DigitalChannel {
  protocol: DigitalProtocol;
  timeslot: number | null;
  /** Talkgroup / GSSI. */
  talkgroup: number | null;
  /** DMR colour code 0..15. */
  colourCode: number | null;
  mcc: number | null;
  mnc: number | null;
  decoderPluginId: string | null;
}

export interface ScanSettings {
  include: boolean;
  /** 0..9. */
  priority: number | null;
  dwellMs: number | null;
}

export interface BookmarkGroup {
  id: string;
  name: string;
  position: number;
}

/** What the user edits; also the unit of bulk import. */
export interface BookmarkInput {
  name: string;
  freqHz: number;
  mode: VfoMode | null;
  bandwidthHz: number | null;
  tags: string[];
  color: string | null;
  notes: string | null;
  /** Squelch threshold in dBFS; null = leave the receiver's squelch as it is. */
  squelchDb: number | null;
  /** The squelch is meant to be set automatically (noise floor); `squelchDb` is the last value. */
  squelchAuto: boolean;
  stepHz: number | null;
  /** null = unspecified (tune leaves the receiver's tone settings alone). */
  toneMode: ToneMode | null;
  ctcssHz: number | null;
  /** DCS code read as three octal digits, kept as that number (023 -> 23). Stored only. */
  dcsCode: number | null;
  dcsPolarity: DcsPolarity | null;
  digital: DigitalChannel | null;
  groupId: string | null;
  favourite: boolean;
  scan: ScanSettings;
}

export interface Bookmark extends BookmarkInput {
  /** Opaque server id (kept as a string whatever the server sends). */
  id: string;
  createdMs: number;
  updatedMs: number;
}

export const NAME_MAX = 100;
export const GROUP_NAME_MAX = 60;
export const BOOKMARK_COLORS: readonly { value: string; label: string }[] = [
  { value: "#fbbf24", label: "Amber" },
  { value: "#f87171", label: "Red" },
  { value: "#fb923c", label: "Orange" },
  { value: "#4ade80", label: "Green" },
  { value: "#2dd4bf", label: "Teal" },
  { value: "#38bdf8", label: "Blue" },
  { value: "#a78bfa", label: "Violet" },
  { value: "#f472b6", label: "Pink" },
];
export const DEFAULT_BOOKMARK_COLOR = "#fbbf24";

const HEX = /^#[0-9a-f]{3,8}$/i;

/** The colour to draw a bookmark in; unknown/invalid values fall back to the default. */
export function bookmarkColor(b: Pick<Bookmark, "color">): string {
  return b.color !== null && HEX.test(b.color) ? b.color : DEFAULT_BOOKMARK_COLOR;
}

export const NO_SCAN: ScanSettings = { include: false, priority: null, dwellMs: null };

/** A complete input from the few fields every bookmark has; everything else is "not set". */
export function newInput(
  base: Pick<BookmarkInput, "name" | "freqHz"> & Partial<BookmarkInput>,
): BookmarkInput {
  return {
    mode: null,
    bandwidthHz: null,
    tags: [],
    color: null,
    notes: null,
    squelchDb: null,
    squelchAuto: false,
    stepHz: null,
    toneMode: null,
    ctcssHz: null,
    dcsCode: null,
    dcsPolarity: null,
    digital: null,
    groupId: null,
    favourite: false,
    scan: NO_SCAN,
    ...base,
  };
}

/** "a, b ,,a" -> ["a","b"]: trimmed, de-duplicated (case-insensitive), empty entries dropped. */
export function parseTags(text: string): string[] {
  const seen = new Set<string>();
  const out: string[] = [];
  for (const part of text.split(/[,;]/)) {
    const tag = part.trim();
    const key = tag.toLowerCase();
    if (tag !== "" && !seen.has(key)) {
      seen.add(key);
      out.push(tag);
    }
  }
  return out;
}

export function sortBookmarks(list: readonly Bookmark[]): Bookmark[] {
  return [...list].sort((a, b) => a.freqHz - b.freqHz || a.name.localeCompare(b.name));
}

export type SortKey = "freq" | "name" | "recent";
export const SORT_LABEL: Record<SortKey, string> = {
  freq: "Frequency",
  name: "Name",
  recent: "Recently changed",
};

/** Sort a list for display; favourites are not special (the panel has a filter for them). */
export function sortBy(list: readonly Bookmark[], key: SortKey): Bookmark[] {
  const byFreq = (a: Bookmark, b: Bookmark) => a.freqHz - b.freqHz || a.name.localeCompare(b.name);
  const out = [...list];
  if (key === "name") out.sort((a, b) => a.name.localeCompare(b.name) || byFreq(a, b));
  else if (key === "recent") out.sort((a, b) => b.updatedMs - a.updatedMs || byFreq(a, b));
  else out.sort(byFreq);
  return out;
}

/** All tags in use, sorted case-insensitively. */
export function allTags(list: readonly Bookmark[]): string[] {
  const seen = new Map<string, string>();
  for (const b of list) {
    for (const t of b.tags) if (!seen.has(t.toLowerCase())) seen.set(t.toLowerCase(), t);
  }
  return [...seen.values()].sort((a, b) => a.localeCompare(b));
}

// --- tones and digital channels, as text -------------------------------------------------------

/** "023N": the DCS code as three digits plus polarity. */
export function formatDcs(code: number, polarity: DcsPolarity | null): string {
  return `${String(code).padStart(3, "0")}${polarity ?? "N"}`;
}

/** Short tone badge text ("88.5", "DCS 023N"), or null without a tone. */
export function toneBadge(b: Pick<Bookmark, "toneMode" | "ctcssHz" | "dcsCode" | "dcsPolarity">) {
  if (b.toneMode === "ctcss" && b.ctcssHz !== null) return b.ctcssHz.toFixed(1);
  if (b.toneMode === "dcs" && b.dcsCode !== null)
    return `DCS ${formatDcs(b.dcsCode, b.dcsPolarity)}`;
  return null;
}

/** Short digital badge text ("TETRA TS3", "DMR TS2 · TG 9"), or null for an analogue channel. */
export function digitalBadge(b: Pick<Bookmark, "digital">): string | null {
  const d = b.digital;
  if (d === null) return null;
  return [
    PROTOCOL_LABEL[d.protocol],
    d.timeslot !== null ? `TS${d.timeslot}` : null,
    d.talkgroup !== null ? `TG ${d.talkgroup}` : null,
  ]
    .filter(Boolean)
    .join(" ");
}

// --- filtering, grouping, proximity --------------------------------------------------------------

/** Group filter: every bookmark, the ungrouped ones, or one group. */
export type GroupFilter = "all" | "none" | (string & {});

export interface Filter {
  query: string;
  tag: string;
  group: GroupFilter;
  favouritesOnly: boolean;
}

export const NO_FILTER: Filter = { query: "", tag: "", group: "all", favouritesOnly: false };

export const isFiltering = (f: Filter): boolean =>
  f.query.trim() !== "" || f.tag !== "" || f.group !== "all" || f.favouritesOnly;

/** Free-text search over name, tags, notes, tone, protocol and the frequency in MHz. */
export function filterBookmarks(
  list: readonly Bookmark[],
  query: string,
  tag: string,
  rest: Partial<Pick<Filter, "group" | "favouritesOnly">> = {},
): Bookmark[] {
  const q = query.trim().toLowerCase();
  const t = tag.toLowerCase();
  const { group = "all", favouritesOnly = false } = rest;
  return list.filter((b) => {
    if (favouritesOnly && !b.favourite) return false;
    if (group === "none" ? b.groupId !== null : group !== "all" && b.groupId !== group) {
      return false;
    }
    if (t !== "" && !b.tags.some((x) => x.toLowerCase() === t)) return false;
    if (q === "") return true;
    const mhz = (b.freqHz / 1e6).toString();
    return (
      b.name.toLowerCase().includes(q) ||
      b.tags.some((x) => x.toLowerCase().includes(q)) ||
      (b.notes ?? "").toLowerCase().includes(q) ||
      (toneBadge(b) ?? "").toLowerCase().includes(q) ||
      (digitalBadge(b) ?? "").toLowerCase().includes(q) ||
      mhz.includes(q)
    );
  });
}

export interface Section {
  /** null = the ungrouped bookmarks. */
  group: BookmarkGroup | null;
  items: Bookmark[];
}

/**
 * Bookmarks per group in the groups' order, the ungrouped ones last. Empty groups are kept unless
 * `dropEmpty` (a filter is active); ungrouped appears only with items.
 */
export function groupSections(
  list: readonly Bookmark[],
  groups: readonly BookmarkGroup[],
  dropEmpty: boolean,
): Section[] {
  const byGroup = new Map<string | null, Bookmark[]>();
  for (const b of list) {
    const key = b.groupId !== null && groups.some((g) => g.id === b.groupId) ? b.groupId : null;
    const items = byGroup.get(key);
    if (items) items.push(b);
    else byGroup.set(key, [b]);
  }
  const sections: Section[] = [];
  for (const group of [...groups].sort((a, b) => a.position - b.position)) {
    const items = byGroup.get(group.id) ?? [];
    if (items.length > 0 || !dropEmpty) sections.push({ group, items });
  }
  const loose = byGroup.get(null) ?? [];
  if (loose.length > 0) sections.push({ group: null, items: loose });
  return sections;
}

/** Bookmarks within `withinHz` of `freqHz`, nearest first, at most `max`. */
export function nearBookmarks(
  list: readonly Bookmark[],
  freqHz: number | null,
  withinHz = 500_000,
  max = 5,
): Bookmark[] {
  if (freqHz === null) return [];
  return list
    .map((b) => ({ b, d: Math.abs(b.freqHz - freqHz) }))
    .filter(({ d }) => d <= withinHz)
    .sort((x, y) => x.d - y.d || x.b.freqHz - y.b.freqHz)
    .slice(0, max)
    .map(({ b }) => b);
}

// --- tuning ------------------------------------------------------------------------------------

/**
 * Everything a click on the bookmark should apply to the receiver. Unset fields are left out so
 * the receiver keeps its own value. The sub-tone is always explicit: a CTCSS bookmark turns the
 * tone squelch on, every other one (DCS is stored only, "no tone", unspecified) turns it off so a
 * tone left over from the previous channel cannot mute this one.
 */
export function bookmarkTarget(b: Bookmark): AnnotationTarget {
  const standardTone =
    b.toneMode === "ctcss" && b.ctcssHz !== null && CTCSS_TONES.includes(b.ctcssHz);
  return {
    freqHz: b.freqHz,
    ...(b.mode ? { mode: b.mode } : {}),
    ...(b.bandwidthHz ? { bandwidthHz: b.bandwidthHz } : {}),
    ...(b.squelchDb !== null ? { squelchDbfs: b.squelchDb } : {}),
    ...(b.squelchAuto ? { squelchAuto: true } : {}),
    ...(b.stepHz ? { stepHz: b.stepHz } : {}),
    toneSquelchHz: standardTone ? b.ctcssHz : null,
  };
}

/** The receiver state worth saving from a VFO (squelch, step, CTCSS tone squelch) as bookmark fields. */
export function receiverState(
  vfo: Vfo | null,
  squelchIsAuto: boolean,
  step: StepChoice,
): Partial<BookmarkInput> {
  const tone = vfo?.toneSquelchHz ?? null;
  return {
    squelchDb: vfo?.squelchDbfs ?? null,
    squelchAuto: squelchIsAuto,
    stepHz: step === "auto" ? null : step,
    ...(tone !== null ? { toneMode: "ctcss" as const, ctcssHz: tone } : {}),
  };
}

/** Identity for merge imports: same frequency and same name (case-insensitive). */
export const bookmarkKey = (b: Pick<BookmarkInput, "name" | "freqHz">): string =>
  `${Math.round(b.freqHz)}|${b.name.trim().toLowerCase()}`;

/** Inputs of `incoming` that are not in `existing` yet (and not repeated within itself). */
export function newBookmarks<T extends Pick<BookmarkInput, "name" | "freqHz">>(
  existing: readonly Pick<BookmarkInput, "name" | "freqHz">[],
  incoming: readonly T[],
): T[] {
  const seen = new Set(existing.map(bookmarkKey));
  const out: T[] = [];
  for (const b of incoming) {
    const key = bookmarkKey(b);
    if (!seen.has(key)) {
      seen.add(key);
      out.push(b);
    }
  }
  return out;
}

export function toInput(b: Bookmark): BookmarkInput {
  return {
    name: b.name,
    freqHz: b.freqHz,
    mode: b.mode,
    bandwidthHz: b.bandwidthHz,
    tags: b.tags,
    color: b.color,
    notes: b.notes,
    squelchDb: b.squelchDb,
    squelchAuto: b.squelchAuto,
    stepHz: b.stepHz,
    toneMode: b.toneMode,
    ctcssHz: b.ctcssHz,
    dcsCode: b.dcsCode,
    dcsPolarity: b.dcsPolarity,
    digital: b.digital,
    groupId: b.groupId,
    favourite: b.favourite,
    scan: b.scan,
  };
}
