// M17 calls as the UI sees them: parsed from the plugin's status document ("who is on the air
// now") and from its call events (the call log). Pure functions, no network (docs/plugins-m17.md).

import { isRec, type Rec } from "../spectrum/api";

export const M17_PLUGIN_ID = "org.superhet.decoder.m17";
export const M17_CALL_START = "m17.call.start";
export const M17_CALL_END = "m17.call.end";

export interface M17Position {
  lat: number;
  lon: number;
  altitudeM: number | null;
  speedKmh: number | null;
  bearingDeg: number | null;
  radiusM: number | null;
  station: string | null;
}

export interface M17Call {
  /** Id of the call (its start time in Unix ms, as a string). */
  id: string;
  src: string;
  dst: string;
  can: number;
  /** voice, voice+data, data, unknown. */
  mode: string;
  /** null when the decoder build does not report the encryption bits. */
  encrypted: boolean | null;
  encryption: string | null;
  text: string | null;
  position: M17Position | null;
  /** The audio of this call is withheld (encrypted). */
  muted: boolean;
  /** Start time (Unix ms) when known (status document). */
  startedMs: number | null;
  /** Set once the call is over: how long it lasted, how much voice was decoded. */
  durationMs: number | null;
  audioMs: number | null;
  /** eos, lost, new-call or stop. */
  reason: string | null;
}

/** What the plugin says it is doing: the call on the air, the last finished one, a counter. */
export interface M17Status {
  now: M17Call | null;
  last: M17Call | null;
  calls: number;
}

export const EMPTY_STATUS: M17Status = { now: null, last: null, calls: 0 };

const str = (v: unknown): string | null => (typeof v === "string" && v !== "" ? v : null);
const num = (v: unknown): number | null => (typeof v === "number" && Number.isFinite(v) ? v : null);

function position(raw: unknown): M17Position | null {
  if (!isRec(raw)) return null;
  const lat = num(raw.lat);
  const lon = num(raw.lon);
  if (lat === null || lon === null) return null;
  if (lat < -90 || lat > 90 || lon < -180 || lon > 180) return null;
  return {
    lat,
    lon,
    altitudeM: num(raw.altitude_m),
    speedKmh: num(raw.speed_kmh),
    bearingDeg: num(raw.bearing_deg),
    radiusM: num(raw.radius_m),
    station: str(raw.station),
  };
}

/** A call from an event's or the status document's data; null when it is not one. */
export function parseCall(raw: unknown): M17Call | null {
  if (!isRec(raw)) return null;
  const src = str(raw.src);
  const dst = str(raw.dst);
  if (src === null || dst === null) return null;
  return {
    id: str(raw.call) ?? "",
    src,
    dst,
    can: num(raw.can) ?? 0,
    mode: str(raw.mode) ?? "unknown",
    encrypted: typeof raw.encrypted === "boolean" ? raw.encrypted : null,
    encryption: str(raw.encryption),
    text: str(raw.text),
    position: position(raw.position),
    muted: raw.muted === true,
    startedMs: num(raw.started_unix_ms),
    durationMs: num(raw.duration_ms),
    audioMs: num(raw.audio_ms),
    reason: str(raw.reason),
  };
}

/** The `status` member of `GET /plugin-instances/{id}/status`. */
export function parseStatus(raw: unknown): M17Status {
  const reply = isRec(raw) ? raw : {};
  const doc: Rec = isRec(reply.status) ? reply.status : {};
  return {
    now: parseCall(doc.now),
    last: parseCall(doc.last),
    calls: num(doc.calls) ?? 0,
  };
}

/** "1.2 s", "34 s", "2 min 05 s". */
export function formatDuration(ms: number): string {
  const total = Math.max(0, Math.round(ms / 100) / 10);
  if (total < 10) return `${total.toFixed(1)} s`;
  const secs = Math.round(total);
  if (secs < 60) return `${secs} s`;
  return `${Math.floor(secs / 60)} min ${String(secs % 60).padStart(2, "0")} s`;
}

/** Human text of an encryption name from the event. */
export function encryptionLabel(call: M17Call): string | null {
  if (call.encrypted === null) return null;
  if (!call.encrypted) return "clear";
  switch (call.encryption) {
    case "aes":
      return "AES encrypted";
    case "scrambler":
      return "scrambled";
    default:
      return "encrypted";
  }
}
