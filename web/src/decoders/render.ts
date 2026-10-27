// How decoder events are shown: a pure model per event (badge, headline, tags, detail line, field
// list, optional map link) that the log rows and the detail dialog render. Built-in detectors
// (source `core`) keep their one-line rows in EventLog; this file covers plugin events
// (docs/plugins-decoders.md) and falls back to a generic key/value view for anything unknown, so a
// new plugin shows up readable without a UI change.

import { isRec, type Rec } from "../spectrum/api";
import type { BadgeTone } from "../ui";
import { CORE_SOURCE, isCoreKind, type DecoderEvent } from "./events";

export interface EventField {
  label: string;
  value: string;
  mono?: boolean;
}

export interface EventLink {
  href: string;
  label: string;
}

export interface EventView {
  /** Short badge text. */
  label: string;
  tone: BadgeTone;
  /** Tooltip of the badge. */
  title: string;
  /** First line: who or what (callsign, RIC, sensor model). */
  headline: string;
  /** Small facts after the headline (type, baud rate, path). */
  tags: string[];
  /** Second line: the content (message text, position, readings). */
  detail: string | null;
  fields: EventField[];
  link: EventLink | null;
  /** Monospace headline (addresses). */
  mono: boolean;
  /** Pretty printed event data for the detail dialog; null if there is none. */
  raw: string | null;
}

/** The plugin's own event data (`meta.data` of a stored event). */
export function eventData(e: DecoderEvent): Rec {
  const d = e.meta.data;
  return isRec(d) ? d : {};
}

const str = (v: unknown): string | null => (typeof v === "string" && v !== "" ? v : null);
const num = (v: unknown): number | null => (typeof v === "number" && Number.isFinite(v) ? v : null);

function put(fields: EventField[], label: string, value: string | number | null, mono = false) {
  if (value === null || value === "") return;
  fields.push({ label, value: String(value), mono });
}

/** Scalar members of `data` that are not in `skip`, as "key: value" fields. */
function leftovers(data: Rec, skip: ReadonlySet<string>): EventField[] {
  const out: EventField[] = [];
  for (const [k, v] of Object.entries(data)) {
    if (skip.has(k)) continue;
    if (typeof v === "string" || typeof v === "number" || typeof v === "boolean") {
      out.push({ label: k, value: String(v), mono: true });
    } else if (Array.isArray(v) && v.every((x) => typeof x !== "object")) {
      out.push({ label: k, value: v.join(", "), mono: true });
    }
  }
  return out;
}

function rawJson(data: Rec): string | null {
  return Object.keys(data).length === 0 ? null : JSON.stringify(data, null, 2);
}

// ---------- POCSAG / FLEX ----------

const POCSAG_FUNCTION: Record<number, string> = {
  0: "0 (numeric)",
  1: "1 (tone 1)",
  2: "2 (tone 2)",
  3: "3 (alphanumeric)",
};

function pocsag(e: DecoderEvent, d: Rec): EventView {
  const address = num(d.address);
  const fn = num(d.function);
  const baud = num(d.baud);
  const kind = str(d.kind);
  const message = typeof d.message === "string" ? d.message : e.value;
  const fields: EventField[] = [];
  put(fields, "Address (RIC)", address, true);
  put(fields, "Function", fn === null ? null : (POCSAG_FUNCTION[fn] ?? String(fn)));
  put(fields, "Type", kind);
  put(fields, "Baud", baud);
  put(fields, "Message", message);
  return {
    label: "POCSAG",
    tone: "warn",
    title: "POCSAG pager message",
    headline: address === null ? "continuation" : `RIC ${address}`,
    tags: [
      ...(fn === null ? [] : [`F${fn}`]),
      ...(kind === null ? [] : [kind]),
      ...(baud === null ? [] : [`${baud} baud`]),
    ],
    detail: message === "" ? "(empty)" : message,
    fields,
    link: null,
    mono: true,
    raw: rawJson(d),
  };
}

function flex(e: DecoderEvent, d: Rec): EventView {
  const capcode = num(d.capcode);
  const group = Array.isArray(d.capcodes) ? d.capcodes.filter((x) => typeof x === "number") : [];
  const baud = num(d.baud);
  const kind = str(d.kind);
  const message = typeof d.message === "string" ? d.message : e.value;
  const fields: EventField[] = [];
  put(fields, "Capcode", capcode, true);
  if (group.length > 1) put(fields, "Group capcodes", group.join(", "), true);
  put(fields, "Type", kind);
  put(fields, "Baud", baud);
  put(fields, "Levels", num(d.levels));
  put(fields, "Cycle", num(d.cycle));
  put(fields, "Frame", num(d.frame));
  put(fields, "Phase", str(d.phase));
  put(fields, "Message", message);
  return {
    label: "FLEX",
    tone: "warn",
    title: "FLEX pager message",
    headline:
      capcode === null
        ? "FLEX"
        : `Capcode ${capcode}${group.length > 1 ? ` +${group.length - 1}` : ""}`,
    tags: [...(kind === null ? [] : [kind]), ...(baud === null ? [] : [`${baud} baud`])],
    detail: message === "" ? "(empty)" : message,
    fields,
    link: null,
    mono: true,
    raw: rawJson(d),
  };
}

// ---------- APRS ----------

/** Names of common APRS symbols: primary table `/`, alternate table `\`. */
const APRS_SYMBOLS: Record<string, string> = {
  "/>": "car",
  "/<": "motorcycle",
  "/-": "house",
  "/#": "digipeater",
  "/_": "weather station",
  "/k": "truck",
  "/u": "truck (18-wheeler)",
  "/v": "van",
  "/j": "jeep",
  "/b": "bicycle",
  "/[": "runner",
  "/O": "balloon",
  "/'": "small aircraft",
  "/^": "large aircraft",
  "/X": "helicopter",
  "/Y": "yacht",
  "/s": "ship",
  "/a": "ambulance",
  "/f": "fire truck",
  "/U": "bus",
  "/R": "recreational vehicle",
  "/r": "antenna",
  "/y": "yagi antenna",
  "/$": "phone",
  "/&": "gateway",
  "/I": "TCP/IP station",
  "/p": "rover",
  "/.": "x",
  "/!": "police",
  "/;": "campground",
  "/%": "DX cluster",
  "\\#": "digipeater (overlay)",
  "\\&": "gateway (overlay)",
  "\\>": "car (overlay)",
  "\\_": "weather station (overlay)",
  "\\-": "house (overlay)",
  "\\n": "node",
};

export function aprsSymbolName(symbol: string): string | null {
  return APRS_SYMBOLS[symbol] ?? null;
}

/** "52.50833, 13.40500" or null if the pair is not a valid position. */
export function formatCoords(lat: number, lon: number): string | null {
  if (lat < -90 || lat > 90 || lon < -180 || lon > 180) return null;
  return `${lat.toFixed(5)}, ${lon.toFixed(5)}`;
}

/** OpenStreetMap link of a position; opens only when the user clicks it. */
export function mapLink(lat: number, lon: number): EventLink | null {
  if (formatCoords(lat, lon) === null) return null;
  const la = lat.toFixed(5);
  const lo = lon.toFixed(5);
  return {
    href: `https://www.openstreetmap.org/?mlat=${la}&mlon=${lo}#map=14/${la}/${lo}`,
    label: "Open map",
  };
}

function aprs(e: DecoderEvent, d: Rec): EventView {
  const from = str(d.source) ?? "?";
  const type = str(d.type) ?? "packet";
  const path = Array.isArray(d.path)
    ? d.path.filter((x): x is string => typeof x === "string")
    : [];
  const lat = num(d.lat);
  const lon = num(d.lon);
  const coords = lat !== null && lon !== null ? formatCoords(lat, lon) : null;
  const comment = str(d.comment);
  const symbol = str(d.symbol);
  const symbolName = symbol === null ? null : aprsSymbolName(symbol);
  const name = str(d.name);

  let detail: string | null;
  if (type === "message") {
    const text = str(d.message) ?? "";
    detail = `to ${str(d.addressee) ?? "?"}: ${text}`;
  } else if (type === "status") {
    detail = str(d.status);
  } else if (coords !== null) {
    detail = comment === null ? coords : `${coords} · ${comment}`;
  } else {
    detail = str(d.info) ?? (e.value === "" ? null : e.value);
  }

  const fields: EventField[] = [];
  put(fields, "From", from, true);
  put(fields, "To", str(d.dest), true);
  put(fields, "Via", path.length === 0 ? null : path.join(", "), true);
  put(fields, "Type", type);
  put(fields, "Name", name);
  if (coords !== null) put(fields, "Position", coords, true);
  if (symbol !== null)
    put(fields, "Symbol", symbolName === null ? symbol : `${symbol} · ${symbolName}`, true);
  put(fields, "Comment", comment);
  const course = num(d.course);
  const speed = num(d.speed_kn);
  const alt = num(d.altitude_m);
  put(fields, "Course", course === null ? null : `${course}°`);
  put(fields, "Speed", speed === null ? null : `${speed} kn`);
  put(fields, "Altitude", alt === null ? null : `${alt} m`);
  put(fields, "Addressee", str(d.addressee), true);
  put(fields, "Message", str(d.message));
  put(fields, "Message id", str(d.message_id), true);
  put(fields, "Status", str(d.status));
  put(fields, "Raw", str(d.info), true);

  return {
    label: "APRS",
    tone: "success",
    title: "APRS packet",
    headline: name !== null ? `${name} (${from})` : from,
    tags: [type, ...(path.length > 0 ? [`via ${path.join(",")}`] : [])],
    detail,
    fields,
    link: lat !== null && lon !== null ? mapLink(lat, lon) : null,
    mono: true,
    raw: rawJson(d),
  };
}

// ---------- rtl_433 ----------

type Reading = readonly [key: string, label: string, format: (v: number) => string];

const READINGS: readonly Reading[] = [
  ["temperature_C", "Temperature", (v) => `${round(v)} °C`],
  ["temperature_F", "Temperature", (v) => `${round(v)} °F`],
  ["humidity", "Humidity", (v) => `${round(v)} %`],
  ["pressure_hPa", "Pressure", (v) => `${round(v)} hPa`],
  ["pressure_kPa", "Pressure", (v) => `${round(v)} kPa`],
  ["pressure_PSI", "Pressure", (v) => `${round(v)} psi`],
  ["wind_avg_km_h", "Wind", (v) => `${round(v)} km/h`],
  ["wind_max_km_h", "Gust", (v) => `${round(v)} km/h`],
  ["wind_dir_deg", "Wind direction", (v) => `${round(v)}°`],
  ["rain_mm", "Rain", (v) => `${round(v)} mm`],
  ["moisture", "Moisture", (v) => `${round(v)} %`],
  ["light_lux", "Light", (v) => `${round(v)} lx`],
  ["uv", "UV", (v) => String(round(v))],
  ["battery_V", "Battery", (v) => `${round(v)} V`],
];

function round(v: number): number {
  return Math.round(v * 100) / 100;
}

function rtl433(e: DecoderEvent, d: Rec): EventView {
  const model = str(d.model) ?? "unknown sensor";
  const id = d.id;
  const idText = typeof id === "number" || typeof id === "string" ? String(id) : null;
  const channel = d.channel;
  const channelText =
    typeof channel === "number" || typeof channel === "string" ? String(channel) : null;
  const battery = d.battery_ok;

  const fields: EventField[] = [];
  put(fields, "Model", model);
  put(fields, "ID", idText, true);
  put(fields, "Channel", channelText);
  const parts: string[] = [];
  const used = new Set(["model", "id", "channel", "summary", "battery_ok"]);
  for (const [key, label, format] of READINGS) {
    const v = num(d[key]);
    used.add(key);
    if (v === null) continue;
    parts.push(format(v));
    put(fields, label, format(v));
  }
  if (typeof battery === "number" || typeof battery === "boolean") {
    const ok = battery === true || battery === 1;
    parts.push(ok ? "battery ok" : "battery low");
    put(fields, "Battery", ok ? "ok" : "low");
  }
  fields.push(...leftovers(d, used));

  return {
    label: "ISM",
    tone: "neutral",
    title: "ISM-band sensor reading (rtl_433)",
    headline: idText === null ? model : `${model} #${idText}`,
    tags: channelText === null ? [] : [`ch ${channelText}`],
    detail: parts.length > 0 ? parts.join(" · ") : e.value || null,
    fields,
    link: null,
    mono: false,
    raw: rawJson(d),
  };
}

// ---------- generic ----------

/** "bytes=1920 seq=47": the scalar members of the data, for a one-line summary. */
function compactFields(d: Rec): string {
  return leftovers(d, new Set(["summary"]))
    .map((f) => `${f.label}=${f.value}`)
    .join(" ");
}

function generic(e: DecoderEvent, d: Rec): EventView {
  const fields: EventField[] = [];
  put(fields, "Source", e.source, true);
  put(fields, "Type", e.kind, true);
  fields.push(...leftovers(d, new Set(["summary"])));
  return {
    label: e.kind.length > 14 ? e.kind.slice(0, 13) + "…" : e.kind,
    tone: "neutral",
    title: `${e.kind} from ${e.source}`,
    // The stored value is the plugin's summary or, without one, the raw JSON of the data.
    headline:
      e.value !== "" && !e.value.startsWith("{")
        ? e.value
        : compactFields(d) || (e.value === "" ? e.kind : e.value),
    tags: [],
    detail: null,
    fields,
    link: null,
    mono: false,
    raw: rawJson(d),
  };
}

/** True for events of the built-in detectors, which keep the compact one-line row. */
export const isCoreEvent = (e: DecoderEvent): boolean =>
  e.source === CORE_SOURCE && isCoreKind(e.kind);

/** The display model of a plugin event; any unknown kind gets the generic view. */
export function describeEvent(e: DecoderEvent): EventView {
  const d = eventData(e);
  switch (e.kind) {
    case "pocsag.message":
      return pocsag(e, d);
    case "flex.message":
      return flex(e, d);
    case "aprs.packet":
      return aprs(e, d);
    case "rtl433.reading":
      return rtl433(e, d);
    default:
      return generic(e, d);
  }
}

/** Height of a log row in px: compact for built-in kinds, two lines for plugin events. */
export const COMPACT_ROW_H = 32;
export const RICH_ROW_H = 52;

export const rowHeight = (e: DecoderEvent): number => (isCoreEvent(e) ? COMPACT_ROW_H : RICH_ROW_H);
