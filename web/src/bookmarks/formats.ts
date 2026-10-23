// Bookmark import/export formats, parsed and written client-side: Superhet JSON, the gqrx
// bookmarks.csv and the SDR++ frequency manager JSON. Modulations without a VFO mode (DSB, raw)
// import without a mode and keep the original name in the notes.

import { VFO_MODES, type VfoMode } from "../vfo/api";
import { DEFAULT_BANDWIDTH_HZ, defaultModeFor } from "../vfo/model";
import { NAME_MAX, parseTags, type Bookmark, type BookmarkInput } from "./model";

export type ImportFormat = "superhet" | "gqrx" | "sdrpp";

export const FORMAT_LABEL: Record<ImportFormat, string> = {
  superhet: "Superhet JSON",
  gqrx: "gqrx bookmarks CSV",
  sdrpp: "SDR++ frequency manager JSON",
};

export interface ImportResult {
  format: ImportFormat;
  bookmarks: BookmarkInput[];
  /** Entries without a usable frequency. */
  skipped: number;
  /** Entries whose modulation has no VFO mode (imported without a mode, noted in the notes). */
  unsupportedMode: number;
}

type Rec = Record<string, unknown>;
const isRec = (v: unknown): v is Rec => typeof v === "object" && v !== null && !Array.isArray(v);
const finite = (v: unknown): v is number => typeof v === "number" && Number.isFinite(v);
const HEX = /^#[0-9a-f]{3,8}$/i;
const GQRX_DEFAULT_COLOR = "#c0c0c0";

function cleanName(raw: string, freqHz: number): string {
  const name = raw.trim().slice(0, NAME_MAX);
  return name === ""
    ? `${(freqHz / 1e6).toFixed(6).replace(/0+$/, "").replace(/\.$/, "")} MHz`
    : name;
}

class Collector {
  readonly bookmarks: BookmarkInput[] = [];
  skipped = 0;
  unsupportedMode = 0;

  add(
    name: string,
    freqHz: unknown,
    mode: VfoMode | null,
    unsupported: string | null,
    bandwidthHz: unknown,
    tags: string[],
    color: string | null,
    notes: string | null,
  ): void {
    if (!finite(freqHz) || freqHz <= 0) {
      this.skipped++;
      return;
    }
    const note = [notes, unsupported ? `Mode: ${unsupported}` : null].filter(Boolean).join("\n");
    if (unsupported) this.unsupportedMode++;
    this.bookmarks.push({
      name: cleanName(name, freqHz),
      freqHz: Math.round(freqHz),
      mode,
      bandwidthHz: finite(bandwidthHz) && bandwidthHz > 0 ? Math.round(bandwidthHz) : null,
      tags,
      color: color !== null && HEX.test(color) ? color : null,
      notes: note === "" ? null : note,
    });
  }

  result(format: ImportFormat): ImportResult {
    return {
      format,
      bookmarks: this.bookmarks,
      skipped: this.skipped,
      unsupportedMode: this.unsupportedMode,
    };
  }
}

// --- gqrx ------------------------------------------------------------------------------------

function gqrxMode(label: string): { mode: VfoMode | null; unsupported: string | null } {
  const m = label.trim().toLowerCase();
  if (m.startsWith("wfm")) return { mode: "wfm", unsupported: null };
  if (m === "narrow fm" || m === "nfm" || m === "fm") return { mode: "nfm", unsupported: null };
  if (m === "am") return { mode: "am", unsupported: null };
  if (m === "usb") return { mode: "usb", unsupported: null };
  if (m === "lsb") return { mode: "lsb", unsupported: null };
  if (m === "cw" || m === "cw-u" || m === "cw-l") return { mode: "cw", unsupported: null };
  return { mode: null, unsupported: m === "" ? null : label.trim() };
}

export function parseGqrx(text: string): ImportResult {
  const colors = new Map<string, string>();
  const out = new Collector();
  for (const line of text.split(/\r?\n/)) {
    const trimmed = line.trim();
    if (trimmed === "" || trimmed.startsWith("#")) continue;
    const f = trimmed.split(";").map((p) => p.trim());
    const freq = Number(f[0]);
    if (f[0] === "" || !Number.isFinite(freq)) {
      // Tag table line: "name ; color".
      if (f[0] && f[1]) colors.set(f[0].toLowerCase(), f[1]);
      continue;
    }
    const { mode, unsupported } = gqrxMode(f[2] ?? "");
    const tags = parseTags(f[4] ?? "").filter((t) => t.toLowerCase() !== "untagged");
    const tagColor = tags.map((t) => colors.get(t.toLowerCase())).find((c) => c !== undefined);
    out.add(
      f[1] ?? "",
      freq,
      mode,
      unsupported,
      Number(f[3]),
      tags,
      tagColor && tagColor.toLowerCase() !== GQRX_DEFAULT_COLOR ? tagColor : null,
      null,
    );
  }
  return out.result("gqrx");
}

const GQRX_LABEL: Record<VfoMode, string> = {
  wfm: "WFM (mono)",
  nfm: "Narrow FM",
  am: "AM",
  usb: "USB",
  lsb: "LSB",
  cw: "CW-U",
};

const csvCell = (s: string) => s.replace(/[;\r\n]+/g, " ").trim();

export function exportGqrx(list: readonly Pick<Bookmark, keyof BookmarkInput>[]): string {
  const colors = new Map<string, string>();
  for (const b of list) {
    for (const t of b.tags) {
      if (!colors.has(t.toLowerCase()) && b.color) colors.set(t.toLowerCase(), b.color);
    }
  }
  const tags = new Map<string, string>();
  for (const b of list)
    for (const t of b.tags) tags.set(t.toLowerCase(), csvCell(t.replace(/,/g, " ")));
  const lines = [
    "# Tag name          ; color",
    `Untagged            ;  ${GQRX_DEFAULT_COLOR}`,
    ...[...tags].map(([key, t]) => `${t} ;  ${colors.get(key) ?? GQRX_DEFAULT_COLOR}`),
    "",
    "# Frequency ; Name ; Modulation ; Bandwidth ; Tags",
    ...list.map((b) => {
      const mode = b.mode ?? defaultModeFor(b.freqHz);
      const bw = b.bandwidthHz ?? DEFAULT_BANDWIDTH_HZ[mode];
      const t = b.tags.map((x) => csvCell(x.replace(/,/g, " "))).join(",");
      return `${Math.round(b.freqHz)} ; ${csvCell(b.name)} ; ${GQRX_LABEL[mode]} ; ${bw} ; ${t || "Untagged"}`;
    }),
    "",
  ];
  return lines.join("\n");
}

// --- SDR++ -----------------------------------------------------------------------------------

/** SDR++ demodulator ids (radio module): 0 NFM, 1 WFM, 2 AM, 3 DSB, 4 USB, 5 CW, 6 LSB, 7 RAW. */
const SDRPP_MODES: readonly (VfoMode | string)[] = [
  "nfm",
  "wfm",
  "am",
  "DSB",
  "usb",
  "cw",
  "lsb",
  "RAW",
];

function sdrppMode(v: unknown): { mode: VfoMode | null; unsupported: string | null } {
  const m = finite(v) ? SDRPP_MODES[v] : undefined;
  if (m === undefined) return { mode: null, unsupported: null };
  const vfo = VFO_MODES.find((x) => x === m);
  return vfo ? { mode: vfo, unsupported: null } : { mode: null, unsupported: m };
}

function sdrppList(out: Collector, tag: string | null, list: unknown): void {
  const books = isRec(list) ? list.bookmarks : undefined;
  if (!isRec(books)) return;
  for (const [name, b] of Object.entries(books)) {
    if (!isRec(b)) {
      out.skipped++;
      continue;
    }
    const { mode, unsupported } = sdrppMode(b.mode);
    out.add(name, b.frequency, mode, unsupported, b.bandwidth, tag ? [tag] : [], null, null);
  }
}

export function parseSdrpp(raw: unknown): ImportResult {
  const out = new Collector();
  if (isRec(raw) && isRec(raw.lists)) {
    for (const [listName, list] of Object.entries(raw.lists)) {
      sdrppList(out, listName === "General" ? null : listName, list);
    }
  } else {
    sdrppList(out, null, raw); // a single exported list
  }
  return out.result("sdrpp");
}

// --- Superhet --------------------------------------------------------------------------------

export function parseSuperhet(raw: unknown): ImportResult {
  const list = Array.isArray(raw) ? raw : isRec(raw) ? raw.bookmarks : undefined;
  if (!Array.isArray(list)) throw new Error("No bookmarks list found in the file.");
  const out = new Collector();
  for (const b of list as unknown[]) {
    if (!isRec(b)) {
      out.skipped++;
      continue;
    }
    const mode = VFO_MODES.find((m) => m === b.mode) ?? null;
    out.add(
      typeof b.name === "string" ? b.name : "",
      b.freq_hz,
      mode,
      null,
      b.bandwidth_hz,
      Array.isArray(b.tags) ? parseTags(b.tags.filter((t) => typeof t === "string").join(",")) : [],
      typeof b.color === "string" ? b.color : null,
      typeof b.notes === "string" && b.notes !== "" ? b.notes : null,
    );
  }
  return out.result("superhet");
}

export function exportSuperhet(list: readonly Pick<Bookmark, keyof BookmarkInput>[]): string {
  return JSON.stringify(
    {
      format: "superhet-bookmarks",
      v: 1,
      bookmarks: list.map((b) => ({
        name: b.name,
        freq_hz: Math.round(b.freqHz),
        mode: b.mode,
        bandwidth_hz: b.bandwidthHz,
        tags: b.tags,
        color: b.color,
        notes: b.notes,
      })),
    },
    null,
    2,
  );
}

/** Detect the format of a file's text and parse it; throws an Error with a user-facing message. */
export function parseImport(text: string): ImportResult {
  const trimmed = text.trim();
  if (trimmed === "") throw new Error("The file is empty.");
  if (trimmed.startsWith("{") || trimmed.startsWith("[")) {
    let raw: unknown;
    try {
      raw = JSON.parse(trimmed);
    } catch {
      throw new Error("The file is not valid JSON.");
    }
    if (isRec(raw) && (isRec(raw.lists) || isRec(raw.bookmarks))) return parseSdrpp(raw);
    return parseSuperhet(raw);
  }
  if (trimmed.includes(";")) return parseGqrx(trimmed);
  throw new Error("Unrecognised file. Expected Superhet JSON, gqrx bookmarks.csv or SDR++ JSON.");
}
