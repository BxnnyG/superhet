// Typed wrappers for the device REST API (docs/api-devices.md) with runtime validation.

import { apiFetch } from "../auth/session";

export interface GainState {
  mode: "auto" | "manual";
  db: number | null;
}

/** One holder of the tuning lock (`docs/api-devices.md`, "Tuning lock"). */
export interface LockHolder {
  /** `user` today; recordings, decoders and claims add their own kinds. */
  kind: string;
  id: string;
  sinceMs: number;
}

/** Usable window around the centre (85 % of the sample rate). */
export interface FreqWindow {
  startHz: number;
  endHz: number;
}

/** Share of the sample rate that is usable (ARCHITECTURE 3). */
export const USABLE_FRACTION = 0.85;

export function usableWindow(centerHz: number, sampleRate: number): FreqWindow {
  const half = (sampleRate * USABLE_FRACTION) / 2;
  return { startHz: centerHz - half, endHz: centerHz + half };
}

export interface DeviceState {
  driver: string;
  tuner?: string;
  centerHz: number;
  sampleRate: number;
  /** null for devices without gain control. */
  gain: GainState | null;
  gainsDb: number[];
  ppm: number;
  biasTee: boolean;
  tuningEpoch: number;
  /** Lock holders; empty = tuning is free. */
  locks: LockHolder[];
  window: FreqWindow;
  /** The ADC clips (front end overloaded); absent from daemons that do not report it. */
  overload?: OverloadState;
}

export interface OverloadState {
  active: boolean;
  /** Share of the samples at the ADC rails over the last few seconds, in percent. */
  clippedPercent: number;
}

export interface TunePatch {
  centerHz?: number;
  sampleRate?: number;
  gain?: "auto" | number;
  ppm?: number;
  biasTee?: boolean;
}

export class ApiError extends Error {
  constructor(
    message: string,
    readonly status?: number,
    readonly code?: string,
    /** Lock holders of a `locked` (409) rejection. */
    readonly holders?: LockHolder[],
  ) {
    super(message);
    this.name = "ApiError";
  }
}

export interface ApiClient {
  getState(device: string, signal?: AbortSignal): Promise<DeviceState>;
  tune(device: string, patch: TunePatch, signal?: AbortSignal): Promise<DeviceState>;
  /** Take (`lock`) or drop (`unlock`) the user tuning lock; both are idempotent. */
  lock(device: string): Promise<DeviceState>;
  unlock(device: string): Promise<DeviceState>;
}

export type Rec = Record<string, unknown>;

export function isRec(v: unknown): v is Rec {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}

function num(o: Rec, key: string): number {
  const v = o[key];
  if (typeof v !== "number" || !Number.isFinite(v)) throw new ApiError(`invalid state: ${key}`);
  return v;
}

function str(o: Rec, key: string): string {
  const v = o[key];
  if (typeof v !== "string") throw new ApiError(`invalid state: ${key}`);
  return v;
}

export function parseHolders(raw: unknown): LockHolder[] {
  if (!Array.isArray(raw)) return [];
  const out: LockHolder[] = [];
  for (const h of raw) {
    if (!isRec(h) || typeof h.kind !== "string") continue;
    out.push({
      kind: h.kind,
      id: typeof h.id === "string" ? h.id : h.kind,
      sinceMs: typeof h.since_ms === "number" ? h.since_ms : 0,
    });
  }
  return out;
}

function parseWindow(raw: unknown, centerHz: number, sampleRate: number): FreqWindow {
  if (
    isRec(raw) &&
    typeof raw.start_hz === "number" &&
    typeof raw.end_hz === "number" &&
    Number.isFinite(raw.start_hz) &&
    Number.isFinite(raw.end_hz) &&
    raw.end_hz > raw.start_hz
  ) {
    return { startHz: raw.start_hz, endHz: raw.end_hz };
  }
  return usableWindow(centerHz, sampleRate);
}

export function parseDeviceState(raw: unknown): DeviceState {
  if (!isRec(raw)) throw new ApiError("invalid state: not an object");
  let gain: GainState | null = null;
  const g = raw.gain;
  if (g !== null && g !== undefined) {
    if (!isRec(g) || (g.mode !== "auto" && g.mode !== "manual")) {
      throw new ApiError("invalid state: gain");
    }
    const db = g.db;
    if (db !== null && db !== undefined && (typeof db !== "number" || !Number.isFinite(db))) {
      throw new ApiError("invalid state: gain.db");
    }
    gain = { mode: g.mode, db: db ?? null };
  }
  const gains = raw.gains_db ?? [];
  if (!Array.isArray(gains) || gains.some((x) => typeof x !== "number" || !Number.isFinite(x))) {
    throw new ApiError("invalid state: gains_db");
  }
  const sampleRate = num(raw, "sample_rate");
  if (sampleRate <= 0) throw new ApiError("invalid state: sample_rate");
  const state: DeviceState = {
    driver: str(raw, "driver"),
    centerHz: num(raw, "center_hz"),
    sampleRate,
    gain,
    gainsDb: [...(gains as number[])].sort((a, b) => a - b),
    ppm: typeof raw.ppm === "number" ? raw.ppm : 0,
    biasTee: raw.bias_tee === true,
    tuningEpoch: typeof raw.tuning_epoch === "number" ? raw.tuning_epoch : 0,
    locks: parseHolders(raw.locks),
    window: parseWindow(raw.window, num(raw, "center_hz"), sampleRate),
  };
  if (typeof raw.tuner === "string") state.tuner = raw.tuner;
  const ov = raw.overload;
  if (isRec(ov) && typeof ov.active === "boolean") {
    const pct = typeof ov.clipped_percent === "number" ? ov.clipped_percent : 0;
    state.overload = { active: ov.active, clippedPercent: Number.isFinite(pct) ? pct : 0 };
  }
  return state;
}

/** Validate a tune reply; an `ok:false` reply becomes an ApiError carrying the code. */
export function parseTuneReply(raw: unknown, status?: number): DeviceState {
  if (!isRec(raw)) throw new ApiError("invalid reply", status);
  if (raw.ok === false) {
    const msg = typeof raw.error === "string" ? raw.error : "tune rejected";
    throw new ApiError(
      msg,
      status,
      typeof raw.code === "string" ? raw.code : undefined,
      parseHolders(raw.holders),
    );
  }
  if (raw.ok !== true) throw new ApiError("invalid reply", status);
  return parseDeviceState(raw.state);
}

export function tuneBody(patch: TunePatch): Rec {
  const body: Rec = { v: 1 };
  if (patch.centerHz !== undefined) body.center_hz = patch.centerHz;
  if (patch.sampleRate !== undefined) body.sample_rate = patch.sampleRate;
  if (patch.gain !== undefined) body.gain = patch.gain;
  if (patch.ppm !== undefined) body.ppm = patch.ppm;
  if (patch.biasTee !== undefined) body.bias_tee = patch.biasTee;
  return body;
}

export async function readJson(res: Response): Promise<unknown> {
  try {
    return await res.json();
  } catch {
    return undefined;
  }
}

export function httpError(res: Response, body: unknown): ApiError {
  if (isRec(body) && typeof body.error === "string") {
    return new ApiError(
      body.error,
      res.status,
      typeof body.code === "string" ? body.code : undefined,
      parseHolders(body.holders),
    );
  }
  return new ApiError(`HTTP ${res.status}`, res.status);
}

export const deviceUrl = (device: string) => `/api/v1/devices/${encodeURIComponent(device)}`;

export function createApiClient(fetchImpl: typeof fetch = apiFetch): ApiClient {
  async function call(url: string, init?: RequestInit): Promise<{ res: Response; body: unknown }> {
    let res: Response;
    try {
      res = await fetchImpl(url, init);
    } catch (e) {
      throw new ApiError(e instanceof Error ? e.message : "network error");
    }
    const body = await readJson(res);
    if (!res.ok) throw httpError(res, body);
    return { res, body };
  }
  return {
    async getState(device, signal) {
      const { body } = await call(`${deviceUrl(device)}/state`, { signal });
      return parseDeviceState(body);
    },
    async tune(device, patch, signal) {
      const { res, body } = await call(`${deviceUrl(device)}/tune`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(tuneBody(patch)),
        signal,
      });
      return parseTuneReply(body, res.status);
    },
    async lock(device) {
      const { res, body } = await call(`${deviceUrl(device)}/lock`, { method: "POST" });
      return parseTuneReply(body, res.status);
    },
    async unlock(device) {
      const { res, body } = await call(`${deviceUrl(device)}/lock`, { method: "DELETE" });
      return parseTuneReply(body, res.status);
    },
  };
}
