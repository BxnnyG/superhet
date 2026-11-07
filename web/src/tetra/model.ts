// TETRA as the UI sees it: parsed from the plugin's status document (cell, calls on the air, whether
// the decoder and the user's ETSI codec are there) and from its events (the call log). Pure
// functions, no network (docs/plugins-tetra.md).

import { formatMhz } from "../decoders/events";
import { isRec, type Rec } from "../spectrum/api";

export const TETRA_PLUGIN_ID = "org.superhet.decoder.tetra";
export const TETRA_CALL_START = "tetra.call.start";
export const TETRA_CALL_END = "tetra.call.end";
export const TETRA_SDS = "tetra.sds";
export const TETRA_STATUS = "tetra.status";
export const TETRA_CELL = "tetra.cell";
export const TETRA_NEIGHBOUR = "tetra.neighbour";

export interface TetraCall {
  /** Id of the call (start time in Unix ms and the call identifier). */
  id: string;
  /** Call identifier of the signalling; null when the set-up was missed. */
  cid: number | null;
  /** Traffic channel (usage marker). */
  usage: number | null;
  /** Called party: the group (GSSI) of a group call. */
  dst: number | null;
  /** Calling party named by the set-up. */
  src: number | null;
  /** Party that last got the right to transmit. */
  talker: number | null;
  encrypted: boolean;
  /** Audio of this call is withheld (encrypted). */
  muted: boolean;
  startedMs: number | null;
  durationMs: number | null;
  voiceMs: number | null;
  audioMs: number | null;
  /** release, lost, new-call or stop. */
  reason: string | null;
  detail: string | null;
}

export interface TetraCell {
  mcc: number;
  mnc: number;
  cc: number;
  la: number | null;
  dlHz: number | null;
  ulHz: number | null;
  /** unknown, none or air-interface. */
  encryption: "unknown" | "none" | "air-interface";
  encryptedSeen: boolean;
  services: string[];
}

export interface TetraNeighbour {
  dlHz: number;
  mcc: number | null;
  mnc: number | null;
  la: number | null;
}

/** Whether tetra-rx was found. */
export interface TetraTool {
  state: "ready" | "missing" | "failed" | "unknown";
  message: string | null;
  searched: string[];
  path: string | null;
}

/** Whether the user's ETSI codec was found and works. */
export interface TetraCodec {
  state: "ready" | "missing" | "failed" | "disabled" | "unknown";
  message: string | null;
  searched: string[];
  detail: string | null;
  dir: string | null;
}

export interface TetraStatus {
  tool: TetraTool;
  codec: TetraCodec;
  /** The decoder sees bursts of a cell. */
  sync: boolean;
  /** Carrier offset the demodulator measured, Hz. */
  offsetHz: number | null;
  cell: TetraCell | null;
  neighbours: TetraNeighbour[];
  now: TetraCall[];
  last: TetraCall | null;
  calls: number;
}

export const EMPTY_STATUS: TetraStatus = {
  tool: { state: "unknown", message: null, searched: [], path: null },
  codec: { state: "unknown", message: null, searched: [], detail: null, dir: null },
  sync: false,
  offsetHz: null,
  cell: null,
  neighbours: [],
  now: [],
  last: null,
  calls: 0,
};

const str = (v: unknown): string | null => (typeof v === "string" && v !== "" ? v : null);
const num = (v: unknown): number | null => (typeof v === "number" && Number.isFinite(v) ? v : null);
const strings = (v: unknown): string[] =>
  Array.isArray(v) ? v.filter((s): s is string => typeof s === "string") : [];

/** A call from an event's or the status document's data; null when it is not one. */
export function parseCall(raw: unknown): TetraCall | null {
  if (!isRec(raw)) return null;
  const id = str(raw.call);
  if (id === null) return null;
  return {
    id,
    cid: num(raw.cid),
    usage: num(raw.usage),
    dst: num(raw.dst),
    src: num(raw.src),
    talker: num(raw.talker),
    encrypted: raw.encrypted === true,
    muted: raw.muted === true,
    startedMs: num(raw.started_unix_ms),
    durationMs: num(raw.duration_ms),
    voiceMs: num(raw.voice_ms),
    audioMs: num(raw.audio_ms),
    reason: str(raw.reason),
    detail: str(raw.detail),
  };
}

/** A cell from an event's or the status document's data. */
export function parseCell(raw: unknown): TetraCell | null {
  if (!isRec(raw)) return null;
  const mcc = num(raw.mcc);
  const mnc = num(raw.mnc);
  if (mcc === null || mnc === null) return null;
  const encryption =
    raw.encryption === "none" || raw.encryption === "air-interface" ? raw.encryption : "unknown";
  return {
    mcc,
    mnc,
    cc: num(raw.cc) ?? 0,
    la: num(raw.la),
    dlHz: num(raw.dl_hz),
    ulHz: num(raw.ul_hz),
    encryption,
    encryptedSeen: raw.encrypted_traffic_seen === true,
    services: strings(raw.services),
  };
}

export function parseNeighbour(raw: unknown): TetraNeighbour | null {
  if (!isRec(raw)) return null;
  const dlHz = num(raw.dl_hz);
  if (dlHz === null) return null;
  return { dlHz, mcc: num(raw.mcc), mnc: num(raw.mnc), la: num(raw.la) };
}

function parseTool(raw: unknown): TetraTool {
  if (!isRec(raw)) return EMPTY_STATUS.tool;
  const state =
    raw.state === "ready" || raw.state === "missing" || raw.state === "failed"
      ? raw.state
      : "unknown";
  return {
    state,
    message: str(raw.message),
    searched: strings(raw.searched),
    path: str(raw.path),
  };
}

function parseCodec(raw: unknown): TetraCodec {
  if (!isRec(raw)) return EMPTY_STATUS.codec;
  const state =
    raw.state === "ready" ||
    raw.state === "missing" ||
    raw.state === "failed" ||
    raw.state === "disabled"
      ? raw.state
      : "unknown";
  return {
    state,
    message: str(raw.message),
    searched: strings(raw.searched),
    detail: str(raw.detail),
    dir: str(raw.dir),
  };
}

/** The `status` member of `GET /plugin-instances/{id}/status`. */
export function parseStatus(raw: unknown): TetraStatus {
  const reply = isRec(raw) ? raw : {};
  const doc: Rec = isRec(reply.status) ? reply.status : {};
  const signal: Rec = isRec(doc.signal) ? doc.signal : {};
  return {
    tool: parseTool(doc.tool),
    codec: parseCodec(doc.codec),
    sync: signal.sync === true,
    offsetHz: num(signal.offset_hz),
    cell: parseCell(doc.cell),
    neighbours: (Array.isArray(doc.neighbours) ? doc.neighbours : [])
      .map(parseNeighbour)
      .filter((n): n is TetraNeighbour => n !== null),
    now: (Array.isArray(doc.now) ? doc.now : [])
      .map(parseCall)
      .filter((c): c is TetraCall => c !== null),
    last: parseCall(doc.last),
    calls: num(doc.calls) ?? 0,
  };
}

/** "262/42": the network as MCC/MNC. */
export const networkLabel = (cell: { mcc: number; mnc: number }): string =>
  `${cell.mcc}/${cell.mnc}`;

/** "262/42 · LA 1 · CC 0 · 392.0125 MHz": what identifies the cell. */
export function cellLabel(cell: TetraCell): string {
  const parts = [networkLabel(cell)];
  if (cell.la !== null) parts.push(`LA ${cell.la}`);
  parts.push(`CC ${cell.cc}`);
  if (cell.dlHz !== null) parts.push(`${formatMhz(cell.dlHz)} MHz`);
  return parts.join(" · ");
}

/** The cell's encryption state in words. */
export function cellEncryptionLabel(cell: TetraCell): string | null {
  if (cell.encryption === "air-interface") return "air interface encryption on";
  if (cell.encryptedSeen) return "encrypted traffic seen";
  return null;
}

/** The calling party, or "?" when the set-up was not heard. */
export const sourceLabel = (call: TetraCall): string =>
  call.src === null ? "?" : String(call.src);

/** "GSSI 9001"-style: the called party (a group for a group call). */
export const targetLabel = (call: TetraCall): string =>
  call.dst === null ? "—" : String(call.dst);

/** "TCH 5": the traffic channel of the call. */
export const channelLabel = (call: TetraCall): string =>
  call.usage === null ? "" : `TCH ${call.usage}`;

/** "1.2 s", "34 s", "2 min 05 s". */
export function formatDuration(ms: number): string {
  const total = Math.max(0, Math.round(ms / 100) / 10);
  if (total < 10) return `${total.toFixed(1)} s`;
  const secs = Math.round(total);
  if (secs < 60) return `${secs} s`;
  return `${Math.floor(secs / 60)} min ${String(secs % 60).padStart(2, "0")} s`;
}

/** Human text of a call's encryption state. */
export const encryptionLabel = (call: TetraCall): string =>
  call.encrypted ? "encrypted" : "clear";
