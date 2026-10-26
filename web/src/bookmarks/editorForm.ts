// Form state of the bookmark editor and the conversion to/from a BookmarkInput. Pure, so the
// validation is tested without the DOM. All fields are text (what the inputs hold).

import { parseFrequency } from "../spectrum/freq";
import { VFO_MODES } from "../vfo/api";
import {
  DIGITAL_PROTOCOLS,
  MAX_TIMESLOT,
  parseTags,
  type BookmarkInput,
  type DcsPolarity,
  type DigitalChannel,
  type DigitalProtocol,
  type ToneMode,
} from "./model";

export type SquelchKind = "keep" | "fixed" | "auto";

export interface FormState {
  name: string;
  /** MHz. */
  freq: string;
  groupId: string;
  mode: string;
  /** kHz. */
  bandwidth: string;
  /** Hz as text; "" = mode default. */
  step: string;
  squelchKind: SquelchKind;
  squelch: string;
  /** "" = not set. */
  toneMode: "" | ToneMode;
  ctcss: string;
  dcsCode: string;
  dcsPolarity: DcsPolarity;
  /** "" = analogue channel. */
  protocol: "" | DigitalProtocol;
  timeslot: string;
  talkgroup: string;
  colourCode: string;
  mcc: string;
  mnc: string;
  decoder: string;
  tags: string;
  color: string;
  favourite: boolean;
  notes: string;
  scanInclude: boolean;
  scanPriority: string;
  scanDwell: string;
}

export type FormErrors = Partial<
  Record<
    | "name"
    | "freq"
    | "bandwidth"
    | "squelch"
    | "ctcss"
    | "dcs"
    | "timeslot"
    | "talkgroup"
    | "colourCode"
    | "mcc"
    | "mnc"
    | "decoder"
    | "scanDwell",
    string
  >
>;

/** MHz text for the field: "433.5", "100.1". */
export function mhzText(hz: number): string {
  return String(parseFloat((hz / 1e6).toFixed(6)));
}

const text = (n: number | null): string => (n === null ? "" : String(n));

export function dcsText(code: number | null): string {
  return code === null ? "" : String(code).padStart(3, "0");
}

export function toForm(i: BookmarkInput): FormState {
  const d = i.digital;
  return {
    name: i.name,
    freq: mhzText(i.freqHz),
    groupId: i.groupId ?? "",
    mode: i.mode ?? "",
    bandwidth: i.bandwidthHz === null ? "" : String(parseFloat((i.bandwidthHz / 1e3).toFixed(3))),
    step: text(i.stepHz),
    squelchKind: i.squelchAuto ? "auto" : i.squelchDb !== null ? "fixed" : "keep",
    squelch: text(i.squelchDb),
    toneMode: i.toneMode ?? "",
    ctcss: text(i.ctcssHz),
    dcsCode: dcsText(i.dcsCode),
    dcsPolarity: i.dcsPolarity ?? "N",
    protocol: d?.protocol ?? "",
    timeslot: text(d?.timeslot ?? null),
    talkgroup: text(d?.talkgroup ?? null),
    colourCode: text(d?.colourCode ?? null),
    mcc: text(d?.mcc ?? null),
    mnc: text(d?.mnc ?? null),
    decoder: d?.decoderPluginId ?? "",
    tags: i.tags.join(", "),
    color: i.color ?? "",
    favourite: i.favourite,
    notes: i.notes ?? "",
    scanInclude: i.scan.include,
    scanPriority: text(i.scan.priority),
    scanDwell: text(i.scan.dwellMs),
  };
}

/** A whole number in `min..=max`, or null for an empty field; `undefined` when invalid. */
function whole(raw: string, min: number, max: number): number | null | undefined {
  const t = raw.trim();
  if (t === "") return null;
  if (!/^\d+$/.test(t)) return undefined;
  const n = Number(t);
  return n >= min && n <= max ? n : undefined;
}

function digitalOf(f: FormState, errors: FormErrors): DigitalChannel | null {
  if (f.protocol === "") return null;
  const protocol = DIGITAL_PROTOCOLS.find((p) => p === f.protocol);
  if (protocol === undefined) return null;
  const slotMax = MAX_TIMESLOT[protocol];
  const timeslot = slotMax > 0 ? whole(f.timeslot, 1, slotMax) : null;
  const talkgroup = whole(f.talkgroup, 0, 16_777_215);
  const colourCode = protocol === "dmr" ? whole(f.colourCode, 0, 15) : null;
  const mcc = whole(f.mcc, 0, 999);
  const mnc = whole(f.mnc, 0, 9999);
  const decoder = f.decoder.trim();
  if (timeslot === undefined) errors.timeslot = `Timeslot 1 to ${slotMax}.`;
  if (talkgroup === undefined) errors.talkgroup = "Whole number up to 16777215.";
  if (colourCode === undefined) errors.colourCode = "Colour code 0 to 15.";
  if (mcc === undefined) errors.mcc = "MCC 0 to 999.";
  if (mnc === undefined) errors.mnc = "MNC 0 to 9999.";
  if (decoder !== "" && !/^[A-Za-z0-9._-]{1,64}$/.test(decoder)) {
    errors.decoder = "Letters, digits, . _ - (up to 64).";
  }
  return {
    protocol,
    timeslot: timeslot ?? null,
    talkgroup: talkgroup ?? null,
    colourCode: colourCode ?? null,
    mcc: mcc ?? null,
    mnc: mnc ?? null,
    decoderPluginId: decoder === "" ? null : decoder,
  };
}

export function fromForm(f: FormState): { input: BookmarkInput } | { errors: FormErrors } {
  const errors: FormErrors = {};
  const name = f.name.trim();
  if (name === "") errors.name = "Enter a name.";
  const freqHz = parseFrequency(f.freq);
  if (freqHz === null) errors.freq = "Enter a frequency, e.g. 433.5 or 145.5M.";

  let bandwidthHz: number | null = null;
  if (f.bandwidth.trim() !== "") {
    const kHz = Number(f.bandwidth.trim().replace(",", "."));
    if (!Number.isFinite(kHz) || kHz <= 0 || kHz > 5_000)
      errors.bandwidth = "Enter kHz, e.g. 12.5.";
    else bandwidthHz = Math.round(kHz * 1000);
  }

  let squelchDb: number | null = null;
  if (f.squelchKind !== "keep" && f.squelch.trim() !== "") {
    const db = Number(f.squelch.trim().replace(",", "."));
    if (!Number.isFinite(db) || db < -120 || db > 0) errors.squelch = "Enter dBFS from -120 to 0.";
    else squelchDb = Math.round(db);
  } else if (f.squelchKind === "fixed") {
    errors.squelch = "Enter dBFS from -120 to 0.";
  }

  let ctcssHz: number | null = null;
  let dcsCode: number | null = null;
  if (f.toneMode === "ctcss") {
    const hz = Number(f.ctcss);
    if (!Number.isFinite(hz) || hz < 30 || hz > 300) errors.ctcss = "Choose a CTCSS tone.";
    else ctcssHz = hz;
  } else if (f.toneMode === "dcs") {
    // Three octal digits (023, 754): kept as that number.
    if (/^[0-7]{1,3}$/.test(f.dcsCode.trim()) && Number(f.dcsCode.trim()) > 0) {
      dcsCode = Number(f.dcsCode.trim());
    } else {
      errors.dcs = "Enter a DCS code of up to three octal digits, e.g. 023.";
    }
  }

  const digital = digitalOf(f, errors);

  const priority = whole(f.scanPriority, 0, 9);
  const dwell = whole(f.scanDwell, 100, 60_000);
  if (dwell === undefined) errors.scanDwell = "Enter ms from 100 to 60000.";

  if (Object.keys(errors).length > 0 || freqHz === null) return { errors };
  const step = whole(f.step, 1, 10_000_000);
  return {
    input: {
      name,
      freqHz,
      mode: VFO_MODES.find((m) => m === f.mode) ?? null,
      bandwidthHz,
      tags: parseTags(f.tags),
      color: f.color === "" ? null : f.color,
      notes: f.notes.trim() === "" ? null : f.notes.trim(),
      squelchDb,
      squelchAuto: f.squelchKind === "auto",
      stepHz: step ?? null,
      toneMode: f.toneMode === "" ? null : f.toneMode,
      ctcssHz,
      dcsCode,
      dcsPolarity: f.toneMode === "dcs" ? f.dcsPolarity : null,
      digital,
      groupId: f.groupId === "" ? null : f.groupId,
      favourite: f.favourite,
      scan: { include: f.scanInclude, priority: priority ?? null, dwellMs: dwell ?? null },
    },
  };
}
