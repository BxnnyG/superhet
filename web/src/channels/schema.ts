// Channel plan model and validation. A plan is a versioned JSON document (docs/channel-plans.md):
// the built-in ones live in `plans/*.json`, custom ones come from the server (operators import
// them). Parsing is strict and throws a `PlanError` naming the offending field, so the import
// dialog can say what is wrong; a plan that fails to parse is never half-loaded.

import { CTCSS_TONES } from "../audio/tones";
import { VFO_MODES, type VfoMode } from "../vfo/api";

/** A channel mode: any VFO mode, or a DAB block (tunes the device centre, no VFO). */
export type ChannelMode = VfoMode | "dab";

export type Licence = "licence-free" | "licensed" | "receive-only";

export const LICENCE_LABEL: Record<Licence, string> = {
  "licence-free": "Licence-free",
  licensed: "Licensed",
  "receive-only": "Receive-only",
};

export type Deemphasis = "50us" | "75us" | "none";

export interface PlanChannel {
  /** Shown on the chip and typed in the palette ("3", "16", "5C", "Guard"). Unique per plan. */
  number: string;
  name: string | null;
  freqHz: number;
  ctcssHz: number | null;
  note: string | null;
  /** Overrides the plan's mode. */
  mode: ChannelMode | null;
  /** Overrides the plan's bandwidth. */
  bandwidthHz: number | null;
}

/**
 * A channel grid without a channel list: `start_hz + i * step_hz`, rounded to whole Hz. Stepping
 * walks it; `icao-8.33` names the channels the way pilots do (118.005, 118.010, 118.015).
 */
export interface Raster {
  id: string;
  label: string;
  startHz: number;
  endHz: number;
  stepHz: number;
  naming: "plain" | "icao-8.33";
}

export interface Plan {
  /** Built-in: the file's id. Custom: `custom:<server id>`. */
  id: string;
  name: string;
  /** Card group and badge ("PMR446", "Marine VHF"). */
  service: string;
  /** Prefix of the readout chip ("PMR" -> "PMR 3"). */
  chip: string;
  /** Lower-case words the palette accepts for this plan ("pmr", "ch"). */
  aliases: string[];
  /** ISO 3166-1 alpha-2 codes or groups (EU, CEPT, ITU-R1..3, WORLD). */
  regions: string[];
  licence: Licence;
  licenceNote: string | null;
  mode: ChannelMode;
  bandwidthHz: number | null;
  deemphasis: Deemphasis | null;
  /** Tuning step to select with a channel (Hz). */
  stepHz: number | null;
  note: string | null;
  source: string | null;
  channels: PlanChannel[];
  rasters: Raster[];
  builtin: boolean;
}

export class PlanError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "PlanError";
  }
}

export const MAX_CHANNELS = 5_000;
export const MAX_RASTERS = 8;
export const MAX_RASTER_COUNT = 20_000;
export const MAX_FREQ_HZ = 10_000_000_000;

type Rec = Record<string, unknown>;
const isRec = (v: unknown): v is Rec => typeof v === "object" && v !== null && !Array.isArray(v);

function fail(path: string, what: string): never {
  throw new PlanError(`${path}: ${what}`);
}

function text(v: unknown, path: string, max: number, required = false): string | null {
  if (v === undefined || v === null || v === "") {
    if (required) fail(path, "required");
    return null;
  }
  if (typeof v !== "string") return fail(path, "must be text");
  const t = v.trim();
  if (t === "") {
    if (required) fail(path, "required");
    return null;
  }
  if (t.length > max) fail(path, `at most ${max} characters`);
  return t;
}

function positive(v: unknown, path: string, max: number): number | null {
  if (v === undefined || v === null) return null;
  if (typeof v !== "number" || !Number.isFinite(v) || v <= 0 || v > max) {
    return fail(path, `must be a number in 1..${max}`);
  }
  return v;
}

function mode(v: unknown, path: string): ChannelMode | null {
  if (v === undefined || v === null) return null;
  if (v === "dab") return "dab";
  const m = VFO_MODES.find((x) => x === v);
  return m ?? fail(path, `unknown mode "${String(v)}"`);
}

/** Comparison key of a channel number: case-insensitive, no spaces, leading zeros dropped. */
export function normalizeNumber(n: string): string {
  const t = n.trim().replace(/\s+/g, "").toLowerCase();
  return /^\d+$/.test(t) ? String(Number(t)) : t;
}

function parseChannel(raw: unknown, i: number, path: string): PlanChannel {
  const at = `${path}[${i}]`;
  if (!isRec(raw)) return fail(at, "must be an object");
  const freq = raw.freq_hz;
  if (typeof freq !== "number" || !Number.isInteger(freq) || freq < 1 || freq > MAX_FREQ_HZ) {
    fail(`${at}.freq_hz`, "must be a whole number of Hz");
  }
  const rawNumber = raw.number;
  const name = text(raw.name, `${at}.name`, 100);
  let number: string;
  if (typeof rawNumber === "number" && Number.isFinite(rawNumber)) number = String(rawNumber);
  else number = text(rawNumber, `${at}.number`, 16) ?? String(i + 1);
  const ctcss = raw.ctcss_hz;
  if (ctcss !== undefined && ctcss !== null) {
    if (typeof ctcss !== "number" || !CTCSS_TONES.includes(ctcss)) {
      fail(`${at}.ctcss_hz`, "not a standard CTCSS tone");
    }
  }
  return {
    number,
    name,
    freqHz: freq as number,
    ctcssHz: typeof ctcss === "number" ? ctcss : null,
    note: text(raw.note, `${at}.note`, 1000),
    mode: mode(raw.mode, `${at}.mode`),
    bandwidthHz: positive(raw.bandwidth_hz, `${at}.bandwidth_hz`, 10_000_000),
  };
}

function parseRaster(raw: unknown, i: number): Raster {
  const at = `rasters[${i}]`;
  if (!isRec(raw)) return fail(at, "must be an object");
  const start = positive(raw.start_hz, `${at}.start_hz`, MAX_FREQ_HZ);
  const end = positive(raw.end_hz, `${at}.end_hz`, MAX_FREQ_HZ);
  const step = positive(raw.step_hz, `${at}.step_hz`, 1e9);
  if (start === null || end === null || step === null) {
    return fail(at, "start_hz, end_hz and step_hz are required");
  }
  if (end < start) fail(`${at}.end_hz`, "must not be below start_hz");
  if ((end - start) / step > MAX_RASTER_COUNT) fail(at, `at most ${MAX_RASTER_COUNT} channels`);
  const naming = raw.naming;
  if (naming !== undefined && naming !== "plain" && naming !== "icao-8.33") {
    fail(`${at}.naming`, 'must be "plain" or "icao-8.33"');
  }
  const label = text(raw.label, `${at}.label`, 40) ?? `${parseFloat((step / 1000).toFixed(3))} kHz`;
  return {
    id: text(raw.id, `${at}.id`, 32) ?? String(i),
    label,
    startHz: start,
    endHz: end,
    stepHz: step,
    naming: naming ?? "plain",
  };
}

const REGION_TOKEN = /^[A-Z0-9][A-Z0-9-]{1,11}$/;

/**
 * Validate a plan document. `id` is the identity to give the plan (custom plans take the server's
 * id; for built-in plans pass `undefined` to use the file's own `id`).
 */
export function parsePlan(raw: unknown, opts: { id?: string; builtin?: boolean } = {}): Plan {
  if (!isRec(raw)) return fail("plan", "must be an object");
  if (raw.v !== 1) fail("v", "must be 1");
  const id = opts.id ?? text(raw.id, "id", 64, true);
  if (id === null || !/^[A-Za-z0-9][A-Za-z0-9:._-]*$/.test(id)) fail("id", "invalid");
  const name = text(raw.name, "name", 100, true) ?? "";
  const service = text(raw.service, "service", 60) ?? name;
  const chip = text(raw.chip, "chip", 12) ?? service.split(/\s+/)[0] ?? name;

  const aliases: string[] = [];
  if (raw.aliases !== undefined) {
    if (!Array.isArray(raw.aliases)) fail("aliases", "must be a list");
    for (const a of raw.aliases) {
      if (typeof a !== "string" || !/^[a-z][a-z0-9]*$/.test(a)) {
        fail("aliases", "lower-case words (letters and digits)");
      }
      aliases.push(a);
    }
  }
  const regions: string[] = [];
  if (raw.regions !== undefined) {
    if (!Array.isArray(raw.regions)) fail("regions", "must be a list");
    for (const r of raw.regions) {
      if (typeof r !== "string" || !REGION_TOKEN.test(r)) {
        fail("regions", "ISO country codes or groups such as EU, in capitals");
      }
      regions.push(r);
    }
  }
  if (regions.length === 0) regions.push("WORLD");

  const licence = raw.licence ?? "receive-only";
  if (licence !== "licence-free" && licence !== "licensed" && licence !== "receive-only") {
    fail("licence", "licence-free, licensed or receive-only");
  }
  const deemphasis = raw.deemphasis ?? null;
  if (
    deemphasis !== null &&
    deemphasis !== "50us" &&
    deemphasis !== "75us" &&
    deemphasis !== "none"
  ) {
    fail("deemphasis", "50us, 75us or none");
  }

  const channels: PlanChannel[] = [];
  if (raw.channels !== undefined && raw.channels !== null) {
    if (!Array.isArray(raw.channels)) fail("channels", "must be a list");
    if (raw.channels.length > MAX_CHANNELS) fail("channels", `at most ${MAX_CHANNELS}`);
    const seen = new Set<string>();
    raw.channels.forEach((c: unknown, i) => {
      const ch = parseChannel(c, i, "channels");
      const key = normalizeNumber(ch.number);
      if (seen.has(key)) fail(`channels[${i}].number`, `duplicate channel "${ch.number}"`);
      seen.add(key);
      channels.push(ch);
    });
  }
  const rasters: Raster[] = [];
  if (raw.rasters !== undefined && raw.rasters !== null) {
    if (!Array.isArray(raw.rasters)) fail("rasters", "must be a list");
    if (raw.rasters.length > MAX_RASTERS) fail("rasters", `at most ${MAX_RASTERS}`);
    raw.rasters.forEach((r: unknown, i) => rasters.push(parseRaster(r, i)));
    if (new Set(rasters.map((r) => r.id)).size !== rasters.length) fail("rasters", "duplicate id");
  }
  if (channels.length === 0 && rasters.length === 0) {
    fail("channels", "a plan needs at least one channel or raster");
  }

  return {
    id,
    name,
    service,
    chip,
    aliases,
    regions,
    licence,
    licenceNote: text(raw.licence_note, "licence_note", 500),
    mode: mode(raw.mode, "mode") ?? "nfm",
    bandwidthHz: positive(raw.bandwidth_hz, "bandwidth_hz", 10_000_000),
    deemphasis,
    stepHz: positive(raw.step_hz, "step_hz", 10_000_000),
    note: text(raw.note, "note", 2000),
    source: text(raw.source, "source", 500),
    channels,
    rasters,
    builtin: opts.builtin === true,
  };
}

/** The effective mode of a channel (its own, else the plan's). */
export const modeOf = (plan: Plan, ch: PlanChannel): ChannelMode => ch.mode ?? plan.mode;

export const bandwidthOf = (plan: Plan, ch: PlanChannel): number | null =>
  ch.bandwidthHz ?? plan.bandwidthHz;

/** HF (up to 30 MHz): an RTL-SDR needs an upconverter or direct sampling there (R820T starts at ~24 MHz and is poor at 27 MHz). */
export const HF_LIMIT_HZ = 30_000_000;

/** Whether the plan lies (partly) in the HF range the usual tuner cannot reach. */
export function needsHf(plan: Plan): boolean {
  return (
    plan.channels.some((c) => c.freqHz < HF_LIMIT_HZ) ||
    plan.rasters.some((r) => r.startHz < HF_LIMIT_HZ)
  );
}

/** Wire form of a plan for the server (the inverse of `parsePlan` for custom plans). */
export function planDocument(plan: Plan): Rec {
  return {
    v: 1,
    name: plan.name,
    service: plan.service,
    chip: plan.chip,
    ...(plan.aliases.length > 0 ? { aliases: plan.aliases } : {}),
    regions: plan.regions,
    licence: plan.licence,
    ...(plan.licenceNote ? { licence_note: plan.licenceNote } : {}),
    mode: plan.mode,
    ...(plan.bandwidthHz ? { bandwidth_hz: plan.bandwidthHz } : {}),
    ...(plan.deemphasis ? { deemphasis: plan.deemphasis } : {}),
    ...(plan.stepHz ? { step_hz: plan.stepHz } : {}),
    ...(plan.note ? { note: plan.note } : {}),
    ...(plan.source ? { source: plan.source } : {}),
    channels: plan.channels.map((c) => ({
      number: c.number,
      ...(c.name ? { name: c.name } : {}),
      freq_hz: c.freqHz,
      ...(c.ctcssHz !== null ? { ctcss_hz: c.ctcssHz } : {}),
      ...(c.note ? { note: c.note } : {}),
      ...(c.mode ? { mode: c.mode } : {}),
      ...(c.bandwidthHz ? { bandwidth_hz: c.bandwidthHz } : {}),
    })),
    ...(plan.rasters.length > 0
      ? {
          rasters: plan.rasters.map((r) => ({
            id: r.id,
            label: r.label,
            start_hz: r.startHz,
            end_hz: r.endHz,
            step_hz: r.stepHz,
            ...(r.naming !== "plain" ? { naming: r.naming } : {}),
          })),
        }
      : {}),
  };
}
