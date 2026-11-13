// Recordings and instant replay (docs/recordings.md, ADR-0028).

import { apiFetch } from "../auth/session";
import { deviceUrl } from "../spectrum/api";

export type RecordingKind = "audio" | "iq";
export type RecordingState = "recording" | "done" | "interrupted" | "failed";

export interface Recording {
  id: number;
  kind: RecordingKind;
  source: "manual" | "replay" | "scanner";
  state: RecordingState;
  title: string;
  device: string;
  vfo: number | null;
  freqHz: number | null;
  mode: string | null;
  /** Only operators and admins get the owner name. */
  owner: string | null;
  startedMs: number;
  stoppedMs: number | null;
  durationMs: number;
  sizeBytes: number;
  keep: boolean;
  meta: Record<string, unknown>;
}

export interface RecordingsConfig {
  available: boolean;
  replaySeconds: number;
  maxBytes: number;
  usedBytes: number;
  quotaExhausted: boolean;
  hitRecording: string[];
}

export interface ListQuery {
  dev?: string;
  kind?: RecordingKind;
  state?: RecordingState;
  keep?: boolean;
  q?: string;
  before?: number;
  limit?: number;
}

export interface StartAudio {
  kind: "audio";
  vfo: number;
  gated: boolean;
  title?: string;
}

export interface StartIq {
  kind: "iq";
  source: "window" | "vfo";
  vfo?: number;
  format?: "cu8" | "cs16" | "cf32";
  rate?: number;
  maxSeconds?: number;
  title?: string;
}

export class RecordingError extends Error {
  constructor(
    message: string,
    readonly status: number,
    readonly code: string,
  ) {
    super(message);
    this.name = "RecordingError";
  }
}

type Rec = Record<string, unknown>;
const isRec = (v: unknown): v is Rec => typeof v === "object" && v !== null && !Array.isArray(v);
const str = (v: unknown): string | null => (typeof v === "string" ? v : null);
const num = (v: unknown): number | null => (typeof v === "number" && Number.isFinite(v) ? v : null);

const KINDS: readonly string[] = ["audio", "iq"];
const STATES: readonly string[] = ["recording", "done", "interrupted", "failed"];

export function parseRecording(raw: unknown): Recording | null {
  if (!isRec(raw)) return null;
  const id = num(raw.id);
  const kind = str(raw.kind);
  const state = str(raw.state);
  if (id === null || kind === null || !KINDS.includes(kind) || state === null) return null;
  if (!STATES.includes(state)) return null;
  const source = raw.source === "replay" || raw.source === "scanner" ? raw.source : "manual";
  return {
    id,
    kind: kind as RecordingKind,
    source,
    state: state as RecordingState,
    title: str(raw.title) ?? "",
    device: str(raw.device) ?? "",
    vfo: num(raw.vfo),
    freqHz: num(raw.freq_hz),
    mode: str(raw.mode),
    owner: str(raw.owner),
    startedMs: num(raw.started_ms) ?? 0,
    stoppedMs: num(raw.stopped_ms),
    durationMs: num(raw.duration_ms) ?? 0,
    sizeBytes: num(raw.size_bytes) ?? 0,
    keep: raw.keep === true,
    meta: isRec(raw.meta) ? raw.meta : {},
  };
}

export function parseConfig(raw: unknown): RecordingsConfig {
  const r = isRec(raw) ? raw : {};
  return {
    available: r.available === true,
    replaySeconds: num(r.replay_seconds) ?? 60,
    maxBytes: num(r.max_bytes) ?? 0,
    usedBytes: num(r.used_bytes) ?? 0,
    quotaExhausted: r.quota_exhausted === true,
    hitRecording: Array.isArray(r.hit_recording)
      ? r.hit_recording.filter((d): d is string => typeof d === "string")
      : [],
  };
}

const TEXT: Record<string, string> = {
  quota_full: "The recordings quota is used up. Delete recordings or raise the quota.",
  empty: "Nothing is buffered for this receiver yet.",
  too_many: "Too many recordings are running.",
  recording_active: "Stop the recording first.",
  not_running: "That recording is not running any more.",
  device_read_only: "This device is read-only for your account.",
  device_forbidden: "This device is not open to your account.",
};

async function call(
  fetchImpl: typeof fetch,
  method: string,
  url: string,
  body?: unknown,
): Promise<Response> {
  let res: Response;
  try {
    res = await fetchImpl(url, {
      method,
      headers: body === undefined ? undefined : { "Content-Type": "application/json" },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
  } catch {
    throw new RecordingError("Cannot reach the server.", 0, "network");
  }
  if (res.ok) return res;
  let parsed: unknown;
  try {
    parsed = await res.json();
  } catch {
    parsed = undefined;
  }
  const rec = isRec(parsed) ? parsed : {};
  const code = str(rec.code) ?? str(rec.error) ?? "";
  const text =
    TEXT[code] ??
    (str(rec.code) !== null ? str(rec.error) : null) ??
    (res.status === 403 ? "Only operators and admins can do that." : null) ??
    `Request failed (HTTP ${res.status}).`;
  throw new RecordingError(text, res.status, code);
}

async function json(res: Response): Promise<unknown> {
  try {
    return await res.json();
  } catch {
    return undefined;
  }
}

export interface RecordingsApi {
  config(): Promise<RecordingsConfig>;
  list(query?: ListQuery): Promise<{ items: Recording[]; nextBefore: number | null }>;
  start(device: string, body: StartAudio | StartIq): Promise<Recording>;
  stop(id: number): Promise<void>;
  update(id: number, patch: { title?: string; keep?: boolean }): Promise<Recording>;
  remove(id: number): Promise<void>;
  /** The last seconds of a VFO as an Ogg Opus blob. */
  replay(device: string, vfo: number, seconds: number): Promise<Blob>;
  saveReplay(device: string, vfo: number, seconds: number): Promise<Recording>;
}

export const fileUrl = (id: number, opts: { download?: boolean; part?: "meta" } = {}): string => {
  const q = new URLSearchParams();
  if (opts.download === true) q.set("download", "true");
  if (opts.part !== undefined) q.set("part", opts.part);
  const qs = q.toString();
  return `/api/v1/recordings/${id}/file${qs === "" ? "" : `?${qs}`}`;
};

export function createRecordingsApi(fetchImpl: typeof fetch = apiFetch): RecordingsApi {
  return {
    async config() {
      return parseConfig(await json(await call(fetchImpl, "GET", "/api/v1/recordings/config")));
    },
    async list(query = {}) {
      const q = new URLSearchParams();
      for (const [k, v] of Object.entries(query)) {
        if (v !== undefined && v !== "") q.set(k, String(v));
      }
      const qs = q.toString();
      const body = await json(
        await call(fetchImpl, "GET", `/api/v1/recordings${qs === "" ? "" : `?${qs}`}`),
      );
      const rec = isRec(body) ? body : {};
      const items = Array.isArray(rec.recordings)
        ? rec.recordings.map(parseRecording).filter((r): r is Recording => r !== null)
        : [];
      return { items, nextBefore: num(rec.next_before) };
    },
    async start(device, body) {
      const wire: Rec =
        body.kind === "audio"
          ? { kind: "audio", vfo: body.vfo, gated: body.gated, title: body.title }
          : {
              kind: "iq",
              source: body.source,
              vfo: body.vfo,
              format: body.format,
              rate: body.rate,
              max_seconds: body.maxSeconds,
              title: body.title,
            };
      const res = await call(fetchImpl, "POST", `${deviceUrl(device)}/recordings`, wire);
      const parsed = parseRecording(await json(res));
      if (parsed === null) throw new RecordingError("Unexpected reply.", res.status, "bad_reply");
      return parsed;
    },
    async stop(id) {
      await call(fetchImpl, "POST", `/api/v1/recordings/${id}/stop`);
    },
    async update(id, patch) {
      const res = await call(fetchImpl, "PATCH", `/api/v1/recordings/${id}`, patch);
      const parsed = parseRecording(await json(res));
      if (parsed === null) throw new RecordingError("Unexpected reply.", res.status, "bad_reply");
      return parsed;
    },
    async remove(id) {
      await call(fetchImpl, "DELETE", `/api/v1/recordings/${id}`);
    },
    async replay(device, vfo, seconds) {
      const res = await call(
        fetchImpl,
        "GET",
        `${deviceUrl(device)}/vfos/${vfo}/replay?seconds=${seconds}`,
      );
      return res.blob();
    },
    async saveReplay(device, vfo, seconds) {
      const res = await call(fetchImpl, "POST", `${deviceUrl(device)}/vfos/${vfo}/replay/save`, {
        seconds,
      });
      const parsed = parseRecording(await json(res));
      if (parsed === null) throw new RecordingError("Unexpected reply.", res.status, "bad_reply");
      return parsed;
    },
  };
}

export const recordingsApi: RecordingsApi = createRecordingsApi();
