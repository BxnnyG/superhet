// Bookmark import/export formats, parsed and written client-side: Superhet JSON (all fields,
// groups), the gqrx bookmarks.csv, the SDR++ frequency manager JSON and CHIRP CSV (tones, step,
// scan skip). Modulations without a VFO mode (DSB, raw, DV) import without a mode and keep the
// original name in the notes.

import { VFO_MODES, type VfoMode } from "../vfo/api";
import { DEFAULT_BANDWIDTH_HZ, defaultModeFor } from "../vfo/model";
import { inputBody, parseBookmark } from "./api";
import {
  NAME_MAX,
  newInput,
  parseTags,
  toInput,
  type Bookmark,
  type BookmarkGroup,
  type BookmarkInput,
  type DcsPolarity,
} from "./model";
import type { ImportItem } from "./store";

export type ImportFormat = "superhet" | "gqrx" | "sdrpp" | "chirp";

export const FORMAT_LABEL: Record<ImportFormat, string> = {
  superhet: "Superhet JSON",
  gqrx: "gqrx bookmarks CSV",
  sdrpp: "SDR++ frequency manager JSON",
  chirp: "CHIRP CSV",
};

export interface ImportResult {
  format: ImportFormat;
  bookmarks: ImportItem[];
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
  readonly bookmarks: ImportItem[] = [];
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
    extra: Partial<BookmarkInput> = {},
  ): void {
    if (!finite(freqHz) || freqHz <= 0) {
      this.skipped++;
      return;
    }
    const note = [notes, unsupported ? `Mode: ${unsupported}` : null].filter(Boolean).join("\n");
    if (unsupported) this.unsupportedMode++;
    this.bookmarks.push(
      newInput({
        ...extra,
        name: cleanName(name, freqHz),
        freqHz: Math.round(freqHz),
        mode,
        bandwidthHz: finite(bandwidthHz) && bandwidthHz > 0 ? Math.round(bandwidthHz) : null,
        tags,
        color: color !== null && HEX.test(color) ? color : null,
        notes: note === "" ? null : note,
      }),
    );
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

// --- CHIRP -----------------------------------------------------------------------------------

const CHIRP_HEADER = [
  "Location",
  "Name",
  "Frequency",
  "Duplex",
  "Offset",
  "Tone",
  "rToneFreq",
  "cToneFreq",
  "DtcsCode",
  "DtcsPolarity",
  "RxDtcsCode",
  "CrossMode",
  "Mode",
  "TStep",
  "Skip",
  "Power",
  "Comment",
  "URCALL",
  "RPT1CALL",
  "RPT2CALL",
  "DVCODE",
];

const CHIRP_MODE: Record<VfoMode, string> = {
  wfm: "WFM",
  nfm: "NFM",
  am: "AM",
  usb: "USB",
  lsb: "LSB",
  cw: "CW",
};

/** One CSV line with optional double quotes (`""` is a quote inside a quoted cell). */
export function parseCsvLine(line: string): string[] {
  const cells: string[] = [];
  let cell = "";
  let quoted = false;
  for (let i = 0; i < line.length; i++) {
    const c = line.charAt(i);
    if (quoted) {
      if (c === '"' && line.charAt(i + 1) === '"') {
        cell += '"';
        i++;
      } else if (c === '"') {
        quoted = false;
      } else {
        cell += c;
      }
    } else if (c === '"') {
      quoted = true;
    } else if (c === ",") {
      cells.push(cell);
      cell = "";
    } else {
      cell += c;
    }
  }
  cells.push(cell);
  return cells;
}

const csvQuote = (s: string) => (/[",\r\n]/.test(s) ? `"${s.replace(/"/g, '""')}"` : s);

function chirpMode(label: string): { mode: VfoMode | null; unsupported: string | null } {
  const m = label.trim().toUpperCase();
  if (m === "FM" || m === "NFM") return { mode: "nfm", unsupported: null };
  if (m === "WFM") return { mode: "wfm", unsupported: null };
  if (m === "AM" || m === "NAM") return { mode: "am", unsupported: null };
  if (m === "USB") return { mode: "usb", unsupported: null };
  if (m === "LSB") return { mode: "lsb", unsupported: null };
  if (m === "CW") return { mode: "cw", unsupported: null };
  return { mode: null, unsupported: m === "" ? null : label.trim() };
}

/** CHIRP imports tones (Tone / TSQL / DTCS), the tuning step, Skip -> scan and the comment. */
export function parseChirp(text: string): ImportResult {
  const lines = text.split(/\r?\n/).filter((l) => l.trim() !== "");
  const head = parseCsvLine(lines[0] ?? "").map((h) => h.trim().toLowerCase());
  const col = (row: string[], name: string) => (row[head.indexOf(name.toLowerCase())] ?? "").trim();
  const out = new Collector();
  for (const line of lines.slice(1)) {
    const row = parseCsvLine(line);
    const { mode, unsupported } = chirpMode(col(row, "Mode"));
    const tone = col(row, "Tone").toUpperCase();
    const extra: Partial<BookmarkInput> = {};
    if (tone === "TSQL" || tone === "TONE") {
      const hz = Number(col(row, tone === "TSQL" ? "cToneFreq" : "rToneFreq"));
      if (Number.isFinite(hz) && hz > 0) {
        extra.toneMode = "ctcss";
        extra.ctcssHz = hz;
      }
    } else if (tone === "DTCS") {
      const code = Number(col(row, "DtcsCode"));
      if (Number.isInteger(code) && code > 0 && code <= 777) {
        const rx: DcsPolarity = col(row, "DtcsPolarity").toUpperCase().endsWith("R") ? "R" : "N";
        extra.toneMode = "dcs";
        extra.dcsCode = code;
        extra.dcsPolarity = rx;
      }
    }
    const stepKhz = Number(col(row, "TStep"));
    if (Number.isFinite(stepKhz) && stepKhz > 0) extra.stepHz = Math.round(stepKhz * 1000);
    extra.scan = { include: col(row, "Skip").toUpperCase() !== "S", priority: null, dwellMs: null };
    const comment = col(row, "Comment");
    out.add(
      col(row, "Name"),
      Number(col(row, "Frequency")) * 1e6,
      mode,
      unsupported,
      null,
      [],
      null,
      comment === "" ? null : comment,
      extra,
    );
  }
  return out.result("chirp");
}

export function exportChirp(list: readonly Pick<Bookmark, keyof BookmarkInput>[]): string {
  const rows = list.map((b, n) => {
    const mode = b.mode ?? defaultModeFor(b.freqHz);
    const ctcss = b.toneMode === "ctcss" && b.ctcssHz !== null;
    const dcs = b.toneMode === "dcs" && b.dcsCode !== null;
    const tone = (b.ctcssHz ?? 88.5).toFixed(1);
    const code = String(b.dcsCode ?? 23).padStart(3, "0");
    const cells = [
      String(n),
      b.name.slice(0, NAME_MAX),
      (b.freqHz / 1e6).toFixed(6),
      "",
      "0.600000",
      ctcss ? "TSQL" : dcs ? "DTCS" : "",
      tone,
      tone,
      code,
      `N${dcs ? (b.dcsPolarity ?? "N") : "N"}`,
      code,
      "Tone->Tone",
      CHIRP_MODE[mode],
      ((b.stepHz ?? 5_000) / 1000).toFixed(2),
      b.scan.include ? "" : "S",
      "5.0W",
      b.notes ?? "",
      "",
      "",
      "",
      "",
    ];
    return cells.map(csvQuote).join(",");
  });
  return [CHIRP_HEADER.join(","), ...rows, ""].join("\n");
}

// --- Superhet --------------------------------------------------------------------------------

/** Version 2 carries every field and the group names; version 1 files (name, frequency, ...) load too. */
export function parseSuperhet(raw: unknown): ImportResult {
  const list = Array.isArray(raw) ? raw : isRec(raw) ? raw.bookmarks : undefined;
  if (!Array.isArray(list)) throw new Error("No bookmarks list found in the file.");
  const out = new Collector();
  for (const b of list as unknown[]) {
    if (!isRec(b)) {
      out.skipped++;
      continue;
    }
    let parsed: Bookmark;
    try {
      parsed = parseBookmark({ ...b, id: "import", group_id: null });
    } catch {
      out.skipped++;
      continue;
    }
    const { name, freqHz, ...rest } = toInput(parsed);
    const color = rest.color !== null && HEX.test(rest.color) ? rest.color : null;
    const group = typeof b.group === "string" ? b.group.trim().slice(0, 60) : "";
    out.bookmarks.push({
      ...newInput({
        ...rest,
        color,
        tags: parseTags(rest.tags.join(",")),
        name: cleanName(name, freqHz),
        freqHz: Math.round(freqHz),
      }),
      ...(group !== "" ? { groupName: group } : {}),
    });
  }
  return out.result("superhet");
}

export function exportSuperhet(
  list: readonly Pick<Bookmark, keyof BookmarkInput>[],
  groups: readonly BookmarkGroup[] = [],
): string {
  const names = new Map(groups.map((g) => [g.id, g.name]));
  return JSON.stringify(
    {
      format: "superhet-bookmarks",
      v: 2,
      groups: groups.map((g) => ({ name: g.name })),
      bookmarks: list.map((b) => {
        const { group_id, ...body } = inputBody(b);
        const group = typeof group_id === "string" ? names.get(group_id) : undefined;
        return group === undefined ? body : { ...body, group };
      }),
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
    if (isRec(raw) && raw.format === "superhet-bookmarks") return parseSuperhet(raw);
    if (isRec(raw) && (isRec(raw.lists) || isRec(raw.bookmarks))) return parseSdrpp(raw);
    return parseSuperhet(raw);
  }
  if (/^location\s*,/i.test(trimmed)) return parseChirp(trimmed);
  if (trimmed.includes(";")) return parseGqrx(trimmed);
  throw new Error(
    "Unrecognised file. Expected Superhet JSON, gqrx bookmarks.csv, SDR++ JSON or CHIRP CSV.",
  );
}
