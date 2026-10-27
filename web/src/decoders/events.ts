// Decoder event history (docs/api-decoder-events.md): the event type, its validation, display
// helpers and the pure reducer behind the log. The store (store.ts) feeds the reducer with REST
// pages and live WebSocket frames; nothing here touches the network.

import { isRec, type Rec } from "../spectrum/api";
import { VFO_MODES, type VfoMode } from "../vfo/api";

/** Kinds of the built-in detectors (source `core`). Plugins add their own event types. */
export const EVENT_KINDS = ["dtmf", "ctcss", "dcs", "rds_ps", "rds_rt", "rds_pi"] as const;
export type CoreKind = (typeof EVENT_KINDS)[number];
/** `source` of the built-in detectors; plugin events carry the plugin id. */
export const CORE_SOURCE = "core";

export const isCoreKind = (kind: string): kind is CoreKind =>
  (EVENT_KINDS as readonly string[]).includes(kind);

export interface DecoderEvent {
  id: number;
  tsMs: number;
  device: string;
  /** null for plugin events of an instance that is not bound to a VFO. */
  vfo: number | null;
  freqHz: number | null;
  mode: VfoMode | null;
  /** A core kind or the event type of a plugin (`pocsag.message`). */
  kind: string;
  /** `core` or the plugin id. */
  source: string;
  value: string;
  meta: Rec;
}

export const KIND_LABEL: Record<CoreKind, string> = {
  dtmf: "DTMF",
  ctcss: "CTCSS",
  dcs: "DCS",
  rds_ps: "PS",
  rds_rt: "RT",
  rds_pi: "PI",
};

export const KIND_HINT: Record<CoreKind, string> = {
  dtmf: "DTMF digit sequence",
  ctcss: "CTCSS sub-tone start / stop",
  dcs: "DCS code",
  rds_ps: "RDS station name (PS)",
  rds_rt: "RDS RadioText (RT)",
  rds_pi: "RDS programme identification (PI)",
};

/** Filter chips: a label and the kinds it switches. */
export const KIND_CHIPS: readonly { label: string; title: string; kinds: readonly CoreKind[] }[] = [
  { label: "DTMF", title: KIND_HINT.dtmf, kinds: ["dtmf"] },
  { label: "CTCSS/DCS", title: "CTCSS sub-tones and DCS codes", kinds: ["ctcss", "dcs"] },
  { label: "PS", title: KIND_HINT.rds_ps, kinds: ["rds_ps"] },
  { label: "RT", title: KIND_HINT.rds_rt, kinds: ["rds_rt"] },
  { label: "PI", title: KIND_HINT.rds_pi, kinds: ["rds_pi"] },
];

/** Events kept in memory; older ones are loaded on demand. */
export const MAX_EVENTS = 2000;

/** Validate one event of the REST API or the live feed; null if it is malformed. */
export function parseEvent(raw: unknown): DecoderEvent | null {
  if (!isRec(raw)) return null;
  const { id, ts_ms, device, vfo, freq_hz, mode, kind, source, value, meta } = raw;
  if (typeof id !== "number" || !Number.isSafeInteger(id)) return null;
  if (typeof ts_ms !== "number" || !Number.isFinite(ts_ms)) return null;
  if (typeof device !== "string") return null;
  // Plugin events of an instance without a VFO have none.
  if (
    vfo !== null &&
    vfo !== undefined &&
    (typeof vfo !== "number" || !Number.isInteger(vfo) || vfo < 0)
  )
    return null;
  if (typeof kind !== "string" || kind === "" || typeof value !== "string") return null;
  // Rows written before plugins existed (and old servers) have no source.
  const src = typeof source === "string" && source !== "" ? source : CORE_SOURCE;
  return {
    id,
    tsMs: ts_ms,
    device,
    vfo: typeof vfo === "number" ? vfo : null,
    freqHz: typeof freq_hz === "number" && Number.isFinite(freq_hz) ? freq_hz : null,
    mode: VFO_MODES.find((m) => m === mode) ?? null,
    kind,
    source: src,
    value,
    meta: isRec(meta) ? meta : {},
  };
}

/** Newest first: by time, then by id. */
export function byNewest(a: DecoderEvent, b: DecoderEvent): number {
  return b.tsMs - a.tsMs || b.id - a.id;
}

/** Union of two lists by id, newest first, capped at `cap` (the oldest fall off). */
export function mergeEvents(
  a: readonly DecoderEvent[],
  b: readonly DecoderEvent[],
  cap = MAX_EVENTS,
): DecoderEvent[] {
  const seen = new Map<number, DecoderEvent>();
  for (const e of a) seen.set(e.id, e);
  for (const e of b) seen.set(e.id, e);
  return [...seen.values()].sort(byNewest).slice(0, cap);
}

export interface LogFilter {
  /** Empty = all kinds. */
  kinds: readonly string[];
  /** `core` or a plugin id; null = every source. */
  source: string | null;
  /** Case-insensitive substring of the value. */
  q: string;
}

export const NO_FILTER: LogFilter = { kinds: [], source: null, q: "" };

export function matches(filter: LogFilter, e: DecoderEvent): boolean {
  if (filter.source !== null && filter.source !== e.source) return false;
  if (filter.kinds.length > 0 && !filter.kinds.includes(e.kind)) return false;
  const q = filter.q.trim().toLowerCase();
  return q === "" || e.value.toLowerCase().includes(q);
}

export interface LogState {
  /** Newest first. */
  events: readonly DecoderEvent[];
  /** Live events held back while paused, newest first. */
  pending: readonly DecoderEvent[];
  paused: boolean;
  filter: LogFilter;
  /** Cursor of the next older page; null when the history is exhausted. */
  next: string | null;
  loading: boolean;
  /** A first page has arrived for the current filter. */
  loaded: boolean;
  error: string | null;
  /** Bumped whenever the query changes; replies of an older generation are dropped. */
  gen: number;
}

export const INITIAL_LOG: LogState = {
  events: [],
  pending: [],
  paused: false,
  filter: NO_FILTER,
  next: null,
  loading: false,
  loaded: false,
  error: null,
  gen: 0,
};

export type LogAction =
  | { type: "query"; filter?: LogFilter }
  | { type: "page"; gen: number; events: readonly DecoderEvent[]; next: string | null }
  | { type: "older"; gen: number; events: readonly DecoderEvent[]; next: string | null }
  | { type: "failed"; gen: number; error: string }
  | { type: "live"; event: DecoderEvent }
  | { type: "pause" }
  | { type: "resume" }
  | { type: "cleared" };

export function logReducer(s: LogState, a: LogAction): LogState {
  switch (a.type) {
    case "query":
      // A new filter starts from scratch; a plain reload (reconnect) keeps what is shown.
      return a.filter === undefined
        ? { ...s, loading: true, error: null, gen: s.gen + 1 }
        : {
            ...s,
            filter: a.filter,
            events: [],
            pending: [],
            next: null,
            loaded: false,
            loading: true,
            error: null,
            gen: s.gen + 1,
          };
    case "page":
      if (a.gen !== s.gen) return s;
      // Live events that arrived during the request stay.
      return {
        ...s,
        events: mergeEvents(s.events, a.events),
        next: a.next,
        loading: false,
        loaded: true,
        error: null,
      };
    case "older":
      if (a.gen !== s.gen) return s;
      return {
        ...s,
        events: mergeEvents(s.events, a.events, MAX_EVENTS * 2),
        next: a.next,
        loading: false,
      };
    case "failed":
      return a.gen !== s.gen ? s : { ...s, loading: false, loaded: true, error: a.error };
    case "live":
      if (!matches(s.filter, a.event)) return s;
      return s.paused
        ? { ...s, pending: mergeEvents(s.pending, [a.event]) }
        : { ...s, events: mergeEvents(s.events, [a.event]) };
    case "pause":
      return s.paused ? s : { ...s, paused: true };
    case "resume":
      return {
        ...s,
        paused: false,
        pending: [],
        events: mergeEvents(s.events, s.pending),
      };
    case "cleared":
      return {
        ...s,
        events: [],
        pending: [],
        next: null,
        loading: false,
        loaded: true,
        error: null,
        gen: s.gen + 1,
      };
  }
}

/** Tuning target of an event: its frequency and, if known, mode. */
export function eventTarget(e: DecoderEvent): { freqHz: number; mode?: VfoMode } | null {
  if (e.freqHz === null || e.freqHz <= 0) return null;
  return e.mode === null ? { freqHz: e.freqHz } : { freqHz: e.freqHz, mode: e.mode };
}

/** "433.5125": MHz with at least three decimals, trailing zeros beyond that trimmed. */
export function formatMhz(hz: number): string {
  const text = (hz / 1e6).toFixed(6);
  const trimmed = text.replace(/0+$/, "");
  const dot = trimmed.indexOf(".");
  return dot < 0 ? trimmed : trimmed.padEnd(dot + 4, "0");
}

const pad = (n: number) => String(n).padStart(2, "0");

/** Clock time for a row; events from another day carry the date. */
export function formatTime(tsMs: number, nowMs = Date.now()): string {
  const d = new Date(tsMs);
  const clock = `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`;
  const today = new Date(nowMs);
  const sameDay =
    d.getFullYear() === today.getFullYear() &&
    d.getMonth() === today.getMonth() &&
    d.getDate() === today.getDate();
  return sameDay ? clock : `${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${clock}`;
}

export function formatFullTime(tsMs: number): string {
  const d = new Date(tsMs);
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(
    d.getMinutes(),
  )}:${pad(d.getSeconds())}`;
}

/** "12.3 s", "4 min 05 s". */
export function formatDuration(ms: number): string {
  const s = Math.max(0, ms) / 1000;
  if (s < 60) return `${s.toFixed(s < 10 ? 1 : 0)} s`;
  const min = Math.floor(s / 60);
  return `${min} min ${pad(Math.floor(s - min * 60))} s`;
}

/** Short note shown next to the value: tone start / stop, digit count. */
export function eventNote(e: DecoderEvent): string | null {
  if (e.source === CORE_SOURCE && (e.kind === "ctcss" || e.kind === "dcs")) {
    if (e.meta.edge === "start") return "on";
    if (e.meta.edge === "stop") {
      const d = e.meta.duration_ms;
      return typeof d === "number" ? `off after ${formatDuration(d)}` : "off";
    }
  }
  return null;
}
