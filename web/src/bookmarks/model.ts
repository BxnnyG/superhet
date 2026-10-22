// Bookmark types and pure helpers (filtering, sorting, tag handling, de-duplication).

import type { VfoMode } from "../vfo/api";

export interface Bookmark {
  /** Opaque server id (kept as a string whatever the server sends). */
  id: string;
  name: string;
  freqHz: number;
  mode: VfoMode | null;
  bandwidthHz: number | null;
  tags: string[];
  color: string | null;
  notes: string | null;
  createdMs: number;
  updatedMs: number;
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
}

export const NAME_MAX = 100;
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

/** All tags in use, sorted case-insensitively. */
export function allTags(list: readonly Bookmark[]): string[] {
  const seen = new Map<string, string>();
  for (const b of list) {
    for (const t of b.tags) if (!seen.has(t.toLowerCase())) seen.set(t.toLowerCase(), t);
  }
  return [...seen.values()].sort((a, b) => a.localeCompare(b));
}

/** Free-text search over name, tags, notes and the frequency in MHz; `tag` "" = any. */
export function filterBookmarks(list: readonly Bookmark[], query: string, tag: string): Bookmark[] {
  const q = query.trim().toLowerCase();
  const t = tag.toLowerCase();
  return list.filter((b) => {
    if (t !== "" && !b.tags.some((x) => x.toLowerCase() === t)) return false;
    if (q === "") return true;
    const mhz = (b.freqHz / 1e6).toString();
    return (
      b.name.toLowerCase().includes(q) ||
      b.tags.some((x) => x.toLowerCase().includes(q)) ||
      (b.notes ?? "").toLowerCase().includes(q) ||
      mhz.includes(q)
    );
  });
}

/** Identity for merge imports: same frequency and same name (case-insensitive). */
export const bookmarkKey = (b: Pick<BookmarkInput, "name" | "freqHz">): string =>
  `${Math.round(b.freqHz)}|${b.name.trim().toLowerCase()}`;

/** Inputs of `incoming` that are not in `existing` yet (and not repeated within itself). */
export function newBookmarks(
  existing: readonly Pick<BookmarkInput, "name" | "freqHz">[],
  incoming: readonly BookmarkInput[],
): BookmarkInput[] {
  const seen = new Set(existing.map(bookmarkKey));
  const out: BookmarkInput[] = [];
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
  };
}
