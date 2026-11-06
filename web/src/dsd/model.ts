// DMR / dPMR / NXDN calls as the UI sees them: parsed from the plugin's status document ("who is
// on the air now", whether dsd-fme is installed) and from its call events (the call log). Pure
// functions, no network (docs/plugins-dsd.md).

import { isRec, type Rec } from "../spectrum/api";

export const DSD_PLUGIN_ID = "org.superhet.decoder.dsd";
export const DSD_CALL_START = "dsd.call.start";
export const DSD_CALL_END = "dsd.call.end";

export type DsdProtocol = "dmr" | "dpmr" | "nxdn";

export interface DsdCall {
  /** Id of the call (its start time in Unix ms, plus the slot for DMR). */
  id: string;
  /** dmr, dpmr, nxdn or unknown. */
  mode: string;
  slot: number | null;
  /** Source (radio) ID. */
  src: number | null;
  /** Talkgroup (group call) or radio ID (private call). */
  dst: number | null;
  group: boolean | null;
  /** DMR / dPMR colour code. */
  cc: number | null;
  /** NXDN radio access number. */
  ran: number | null;
  encrypted: boolean;
  algorithm: string | null;
  /** The audio of this call is withheld (encrypted). */
  muted: boolean;
  /** Start time (Unix ms) when known (status document). */
  startedMs: number | null;
  /** Set once the call is over. */
  durationMs: number | null;
  audioMs: number | null;
  /** eos, lost, new-call or stop. */
  reason: string | null;
}

/** Whether the user-installed dsd-fme was found. */
export interface DsdTool {
  /** ready, missing, failed; unknown for a document without the member. */
  state: "ready" | "missing" | "failed" | "unknown";
  message: string | null;
  searched: string[];
  path: string | null;
}

export interface DsdStatus {
  tool: DsdTool;
  /** Calls on the air (DMR: up to one per slot). */
  now: DsdCall[];
  last: DsdCall | null;
  calls: number;
}

export const EMPTY_STATUS: DsdStatus = {
  tool: { state: "unknown", message: null, searched: [], path: null },
  now: [],
  last: null,
  calls: 0,
};

const str = (v: unknown): string | null => (typeof v === "string" && v !== "" ? v : null);
const num = (v: unknown): number | null => (typeof v === "number" && Number.isFinite(v) ? v : null);

/** A call from an event's or the status document's data; null when it is not one. */
export function parseCall(raw: unknown): DsdCall | null {
  if (!isRec(raw)) return null;
  const src = num(raw.src);
  const dst = num(raw.dst);
  // A call is known by who talks or to whom; link control may name only one of them.
  if (src === null && dst === null) return null;
  return {
    id: str(raw.call) ?? "",
    mode: str(raw.mode) ?? "unknown",
    slot: num(raw.slot),
    src,
    dst,
    group: typeof raw.group === "boolean" ? raw.group : null,
    cc: num(raw.cc),
    ran: num(raw.ran),
    encrypted: raw.encrypted === true,
    algorithm: str(raw.algorithm),
    muted: raw.muted === true,
    startedMs: num(raw.started_unix_ms),
    durationMs: num(raw.duration_ms),
    audioMs: num(raw.audio_ms),
    reason: str(raw.reason),
  };
}

function parseTool(raw: unknown): DsdTool {
  if (!isRec(raw)) return EMPTY_STATUS.tool;
  const state =
    raw.state === "ready" || raw.state === "missing" || raw.state === "failed"
      ? raw.state
      : "unknown";
  return {
    state,
    message: str(raw.message),
    searched: Array.isArray(raw.searched)
      ? raw.searched.filter((s): s is string => typeof s === "string")
      : [],
    path: str(raw.path),
  };
}

/** The `status` member of `GET /plugin-instances/{id}/status`. */
export function parseStatus(raw: unknown): DsdStatus {
  const reply = isRec(raw) ? raw : {};
  const doc: Rec = isRec(reply.status) ? reply.status : {};
  return {
    tool: parseTool(doc.tool),
    now: (Array.isArray(doc.now) ? doc.now : [])
      .map(parseCall)
      .filter((c): c is DsdCall => c !== null),
    last: parseCall(doc.last),
    calls: num(doc.calls) ?? 0,
  };
}

export const PROTOCOL_LABEL: Record<string, string> = {
  dmr: "DMR",
  dpmr: "dPMR",
  nxdn: "NXDN",
};

export const protocolLabel = (mode: string): string => PROTOCOL_LABEL[mode] ?? "Digital voice";

/** "TG 91" for a group call, "ID 4000123" for a private one, "—" when unknown. */
export function targetLabel(call: DsdCall): string {
  if (call.dst === null) return "—";
  return `${call.group === false ? "ID" : "TG"} ${call.dst}`;
}

export const sourceLabel = (call: DsdCall): string => (call.src === null ? "?" : String(call.src));

/** "slot 1 · CC 1", "RAN 4": the channel facts that fit the protocol. */
export function channelLabel(call: DsdCall): string {
  const parts: string[] = [];
  if (call.slot !== null) parts.push(`slot ${call.slot}`);
  if (call.cc !== null) parts.push(`CC ${call.cc}`);
  if (call.ran !== null) parts.push(`RAN ${call.ran}`);
  return parts.join(" · ");
}

/** "1.2 s", "34 s", "2 min 05 s". */
export function formatDuration(ms: number): string {
  const total = Math.max(0, Math.round(ms / 100) / 10);
  if (total < 10) return `${total.toFixed(1)} s`;
  const secs = Math.round(total);
  if (secs < 60) return `${secs} s`;
  return `${Math.floor(secs / 60)} min ${String(secs % 60).padStart(2, "0")} s`;
}

/** Human text of the encryption state. */
export function encryptionLabel(call: DsdCall): string {
  if (!call.encrypted) return "clear";
  return call.algorithm === null ? "encrypted" : `encrypted (${call.algorithm})`;
}
