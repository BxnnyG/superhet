// Typed wrappers for the device REST API (docs/api-devices.md) with runtime validation.

import { apiFetch } from "../auth/session";

export interface GainState {
  mode: "auto" | "manual";
  db: number | null;
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
  ) {
    super(message);
    this.name = "ApiError";
  }
}

export interface ApiClient {
  getState(device: string, signal?: AbortSignal): Promise<DeviceState>;
  tune(device: string, patch: TunePatch, signal?: AbortSignal): Promise<DeviceState>;
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
  };
  if (typeof raw.tuner === "string") state.tuner = raw.tuner;
  return state;
}

/** Validate a tune reply; an `ok:false` reply becomes an ApiError carrying the code. */
export function parseTuneReply(raw: unknown, status?: number): DeviceState {
  if (!isRec(raw)) throw new ApiError("invalid reply", status);
  if (raw.ok === false) {
    const msg = typeof raw.error === "string" ? raw.error : "tune rejected";
    throw new ApiError(msg, status, typeof raw.code === "string" ? raw.code : undefined);
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
    );
  }
  return new ApiError(`HTTP ${res.status}`, res.status);
}

export const deviceUrl = (device: string) => `/api/v1/devices/${encodeURIComponent(device)}`;

export function createApiClient(fetchImpl: typeof fetch = apiFetch): ApiClient {
  return {
    async getState(device, signal) {
      let res: Response;
      try {
        res = await fetchImpl(`${deviceUrl(device)}/state`, { signal });
      } catch (e) {
        throw new ApiError(e instanceof Error ? e.message : "network error");
      }
      const body = await readJson(res);
      if (!res.ok) throw httpError(res, body);
      return parseDeviceState(body);
    },
    async tune(device, patch, signal) {
      let res: Response;
      try {
        res = await fetchImpl(`${deviceUrl(device)}/tune`, {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify(tuneBody(patch)),
          signal,
        });
      } catch (e) {
        throw new ApiError(e instanceof Error ? e.message : "network error");
      }
      const body = await readJson(res);
      if (!res.ok) throw httpError(res, body);
      return parseTuneReply(body, res.status);
    },
  };
}
