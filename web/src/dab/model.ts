// DAB / DAB+ plugin (org.superhet.decoder.dab, docs/plugins-dab.md): the state document the
// plugin reports through `GET /api/v1/plugin-instances/{id}/status`, parsed defensively, plus the
// Band III block table and small view helpers.

import { ApiError, isRec } from "../spectrum/api";
import { INSTANCE_STATES, type InstanceState } from "../plugins/api";

export const DAB_PLUGIN_ID = "org.superhet.decoder.dab";

/** Band III blocks, ascending: name and centre frequency (Hz). Same table as the channel plan. */
export const DAB_BLOCKS: readonly (readonly [string, number])[] = [
  ["5A", 174_928_000],
  ["5B", 176_640_000],
  ["5C", 178_352_000],
  ["5D", 180_064_000],
  ["6A", 181_936_000],
  ["6B", 183_648_000],
  ["6C", 185_360_000],
  ["6D", 187_072_000],
  ["7A", 188_928_000],
  ["7B", 190_640_000],
  ["7C", 192_352_000],
  ["7D", 194_064_000],
  ["8A", 195_936_000],
  ["8B", 197_648_000],
  ["8C", 199_360_000],
  ["8D", 201_072_000],
  ["9A", 202_928_000],
  ["9B", 204_640_000],
  ["9C", 206_352_000],
  ["9D", 208_064_000],
  ["10A", 209_936_000],
  ["10B", 211_648_000],
  ["10C", 213_360_000],
  ["10D", 215_072_000],
  ["11A", 216_928_000],
  ["11B", 218_640_000],
  ["11C", 220_352_000],
  ["11D", 222_064_000],
  ["12A", 223_936_000],
  ["12B", 225_648_000],
  ["12C", 227_360_000],
  ["12D", 229_072_000],
  ["13A", 230_784_000],
  ["13B", 232_496_000],
  ["13C", 234_208_000],
  ["13D", 235_776_000],
  ["13E", 237_488_000],
  ["13F", 239_200_000],
];

/** Sample rate the plugin needs (the claim sets it). */
export const DAB_SAMPLE_RATE = 2_048_000;

export function blockFreq(name: string): number | null {
  return DAB_BLOCKS.find(([n]) => n === name)?.[1] ?? null;
}

export function blockOf(freqHz: number | null): string | null {
  if (freqHz === null) return null;
  return DAB_BLOCKS.find(([, f]) => Math.abs(f - freqHz) <= 50_000)?.[0] ?? null;
}

export interface DabService {
  /** Service id, four (or eight) upper-case hex digits. */
  sid: string;
  label: string;
  pty: string;
  language: string;
  codec: string;
  bitrate: number | null;
  protection: string;
  /** Whether the service has an audio stream. */
  audio: boolean;
  /** Now playing text; only for the selected service. */
  dls: string | null;
  audioErrors: number | null;
}

export interface DabEnsemble {
  label: string;
  eid: string;
  ecc: string;
}

/** An ensemble found on a block. */
export interface DabFound {
  channel: string;
  freqHz: number | null;
  label: string;
  eid: string;
  ecc: string;
  services: number;
  snr: number | null;
}

export interface DabScan {
  index: number;
  total: number;
  channel: string | null;
  found: DabFound[];
}

export interface DabStatus {
  mode: "idle" | "tuned" | "scanning";
  channel: string | null;
  freqHz: number | null;
  receiverRunning: boolean;
  /** An ensemble id was decoded. */
  synced: boolean;
  snr: number | null;
  ficCrcErrors: number | null;
  iqConnected: boolean;
  ensemble: DabEnsemble | null;
  services: DabService[];
  /** Service id selected for playing. */
  selected: string | null;
  playing: boolean;
  scan: DabScan | null;
  lastScan: { finishedMs: number; found: DabFound[] } | null;
  error: string | null;
}

export interface DabMedia {
  key: string;
  mime: string;
  size: number;
  revision: number;
}

/** The reply of `GET /api/v1/plugin-instances/{id}/status`. */
export interface DabReply {
  pluginId: string;
  device: string | null;
  state: InstanceState;
  freqHz: number | null;
  /** Pseudo VFO id of the audio stream. */
  audioVfo: number | null;
  /** Null until the plugin reported for the first time. */
  status: DabStatus | null;
  updatedMs: number;
  media: DabMedia[];
}

const str = (v: unknown, d = ""): string => (typeof v === "string" ? v : d);
const strOrNull = (v: unknown): string | null => (typeof v === "string" && v !== "" ? v : null);
const numOrNull = (v: unknown): number | null =>
  typeof v === "number" && Number.isFinite(v) ? v : null;
const list = (v: unknown): unknown[] => (Array.isArray(v) ? v : []);

function parseService(raw: unknown): DabService | null {
  if (!isRec(raw) || typeof raw.sid !== "string") return null;
  return {
    sid: raw.sid.toUpperCase(),
    label: str(raw.label),
    pty: str(raw.pty),
    language: str(raw.language),
    codec: str(raw.codec),
    bitrate: numOrNull(raw.bitrate),
    protection: str(raw.protection),
    audio: raw.audio === true,
    dls: strOrNull(raw.dls),
    audioErrors: numOrNull(raw.audio_errors),
  };
}

export function parseFound(raw: unknown): DabFound | null {
  if (!isRec(raw) || typeof raw.channel !== "string") return null;
  return {
    channel: raw.channel,
    freqHz: numOrNull(raw.freq_hz),
    label: str(raw.label),
    eid: str(raw.eid),
    ecc: str(raw.ecc),
    services: numOrNull(raw.services) ?? 0,
    snr: numOrNull(raw.snr),
  };
}

const foundList = (v: unknown): DabFound[] =>
  list(v)
    .map(parseFound)
    .filter((f): f is DabFound => f !== null);

export function parseStatus(raw: unknown): DabStatus | null {
  if (!isRec(raw) || raw.v !== 1) return null;
  const mode = raw.mode === "tuned" || raw.mode === "scanning" ? raw.mode : "idle";
  const ens = isRec(raw.ensemble) ? raw.ensemble : null;
  const scan = isRec(raw.scan) ? raw.scan : null;
  const last = isRec(raw.last_scan) ? raw.last_scan : null;
  return {
    mode,
    channel: strOrNull(raw.channel),
    freqHz: numOrNull(raw.freq_hz),
    receiverRunning: raw.receiver_running === true,
    synced: raw.synced === true,
    snr: numOrNull(raw.snr),
    ficCrcErrors: numOrNull(raw.fic_crc_errors),
    iqConnected: raw.iq_connected === true,
    ensemble: ens ? { label: str(ens.label), eid: str(ens.eid), ecc: str(ens.ecc) } : null,
    services: list(raw.services)
      .map(parseService)
      .filter((s): s is DabService => s !== null),
    selected: strOrNull(raw.selected)?.toUpperCase() ?? null,
    playing: raw.playing === true,
    scan: scan
      ? {
          index: numOrNull(scan.index) ?? 0,
          total: numOrNull(scan.total) ?? 0,
          channel: strOrNull(scan.channel),
          found: foundList(scan.found),
        }
      : null,
    lastScan: last
      ? { finishedMs: numOrNull(last.finished_unix_ms) ?? 0, found: foundList(last.found) }
      : null,
    error: strOrNull(raw.error),
  };
}

export function parseReply(raw: unknown): DabReply {
  if (!isRec(raw) || typeof raw.plugin_id !== "string") throw new ApiError("invalid status reply");
  return {
    pluginId: raw.plugin_id,
    device: strOrNull(raw.device),
    state: INSTANCE_STATES.find((s) => s === raw.state) ?? "error",
    freqHz: numOrNull(raw.freq_hz),
    audioVfo: numOrNull(raw.audio_vfo),
    status: parseStatus(raw.status),
    updatedMs: numOrNull(raw.updated_unix_ms) ?? 0,
    media: list(raw.media)
      .filter(isRec)
      .map((m) => ({
        key: str(m.key),
        mime: str(m.mime),
        size: numOrNull(m.size) ?? 0,
        revision: numOrNull(m.revision) ?? 0,
      }))
      .filter((m) => m.key !== ""),
  };
}

export type SignalGrade = "none" | "poor" | "fair" | "good";

/** Rough reception quality for the badge: SNR of the OFDM demodulator, errors in the FIC. */
export function signalGrade(status: DabStatus | null): SignalGrade {
  if (status === null || !status.synced) return "none";
  const snr = status.snr ?? 0;
  if (snr >= 12) return "good";
  if (snr >= 6) return "fair";
  return "poor";
}

/** Key of the slideshow object of a service (`slide.<sid>` in lower case). */
export const slideKey = (sid: string): string => `slide.${sid.toLowerCase()}`;

/** Newest ensemble per block from `dab.ensemble` event history (newest first in, newest wins). */
export function latestEnsembles(
  events: readonly { kind: string; meta: Record<string, unknown> }[],
): DabFound[] {
  const seen = new Map<string, DabFound>();
  for (const e of events) {
    if (e.kind !== "dab.ensemble") continue;
    const data = isRec(e.meta.data) ? e.meta.data : null;
    const found = data ? parseFound({ ...data, services: list(data.services).length }) : null;
    if (found && !seen.has(found.channel)) seen.set(found.channel, found);
  }
  return [...seen.values()].sort(
    (a, b) => (blockFreq(a.channel) ?? 0) - (blockFreq(b.channel) ?? 0),
  );
}
