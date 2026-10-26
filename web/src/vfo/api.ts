// Typed wrappers for the VFO REST routes (docs/api-devices.md) with runtime validation.

import { apiFetch } from "../auth/session";
import {
  ApiError,
  deviceUrl,
  httpError,
  isRec,
  parseDeviceState,
  parseHolders,
  readJson,
  type DeviceState,
  type Rec,
} from "../spectrum/api";

export type VfoMode = "wfm" | "nfm" | "am" | "usb" | "lsb" | "cw";
export const VFO_MODES: readonly VfoMode[] = ["wfm", "nfm", "am", "usb", "lsb", "cw"];

export function isVfoMode(v: unknown): v is VfoMode {
  return VFO_MODES.some((m) => m === v);
}

export interface Vfo {
  id: number;
  freqHz: number;
  mode: VfoMode;
  /** Effective bandwidth. */
  bandwidthHz: number;
  /** null = squelch off. */
  squelchDbfs: number | null;
  deemphasis: string | null;
  /** CW pitch in Hz; null for every mode but CW. */
  cwPitchHz?: number | null;
  /** Stereo decoding on/off; null for modes without stereo (everything but WFM). */
  stereo?: boolean | null;
  /** CTCSS / DTMF detection on/off; null for modes without it (everything but NFM and AM). */
  tones?: boolean | null;
  /** CTCSS tone squelch in Hz; null = off. */
  toneSquelchHz?: number | null;
  offsetHz: number;
  status: "active" | "paused";
}

export interface VfoCreate {
  freqHz: number;
  mode: VfoMode;
  bandwidthHz?: number;
  squelchDbfs?: number | null;
  /** CW only: pitch in Hz (daemon default 700). */
  cwPitchHz?: number;
  /** NFM and AM only, one of the standard CTCSS tones. */
  toneSquelchHz?: number | null;
}

/** Partial update; `null` resets bandwidth (mode default) or switches the squelch off. */
export interface VfoPatch {
  freqHz?: number;
  mode?: VfoMode;
  bandwidthHz?: number | null;
  squelchDbfs?: number | null;
  /** CW only: pitch in Hz (200..2000 on the daemon); `null` resets to the default. */
  cwPitchHz?: number | null;
  /** WFM only; `null` resets to the default (on). */
  stereo?: boolean | null;
  /** NFM and AM only; `null` resets to the default (on for NFM, off for AM). */
  tones?: boolean | null;
  /** NFM and AM only, one of the standard CTCSS tones; `null` switches the tone squelch off. */
  toneSquelchHz?: number | null;
}

/**
 * What the daemon did about a VFO that ended up outside the usable window (free tuning):
 * `none` inside the window, `applied` the centre followed, `locked` the tuning lock kept the
 * centre (VFO paused), `failed` the device refused the retune.
 */
export type Retune = "none" | "applied" | "locked" | "failed";

export interface VfoReply {
  vfo: Vfo;
  /** All VFOs of the device after the change (from the reply's state document), if present. */
  vfos: Vfo[] | null;
  /** The device state of the reply (centre may have moved), if present. */
  state: DeviceState | null;
  retune: Retune;
}

export interface VfoApi {
  list(device: string, signal?: AbortSignal): Promise<Vfo[]>;
  create(device: string, req: VfoCreate): Promise<VfoReply>;
  update(device: string, id: number, patch: VfoPatch): Promise<VfoReply>;
  remove(device: string, id: number): Promise<void>;
}

function bad(what: string): never {
  throw new ApiError(`invalid vfo: ${what}`);
}

export function parseVfo(raw: unknown): Vfo {
  if (!isRec(raw)) return bad("not an object");
  const {
    id,
    freq_hz,
    mode,
    bandwidth_hz,
    squelch_dbfs,
    deemphasis,
    cw_pitch_hz,
    stereo,
    tones,
    tone_squelch_hz,
    offset_hz,
    status,
  } = raw;
  if (typeof id !== "number" || !Number.isInteger(id) || id < 1) return bad("id");
  if (typeof freq_hz !== "number" || !Number.isFinite(freq_hz)) return bad("freq_hz");
  if (!isVfoMode(mode)) return bad("mode");
  if (typeof bandwidth_hz !== "number" || !Number.isFinite(bandwidth_hz))
    return bad("bandwidth_hz");
  if (
    squelch_dbfs !== null &&
    squelch_dbfs !== undefined &&
    (typeof squelch_dbfs !== "number" || !Number.isFinite(squelch_dbfs))
  ) {
    return bad("squelch_dbfs");
  }
  return {
    id,
    freqHz: freq_hz,
    mode,
    bandwidthHz: bandwidth_hz,
    squelchDbfs: typeof squelch_dbfs === "number" ? squelch_dbfs : null,
    deemphasis: typeof deemphasis === "string" ? deemphasis : null,
    cwPitchHz: typeof cw_pitch_hz === "number" && Number.isFinite(cw_pitch_hz) ? cw_pitch_hz : null,
    stereo: typeof stereo === "boolean" ? stereo : null,
    tones: typeof tones === "boolean" ? tones : null,
    toneSquelchHz:
      typeof tone_squelch_hz === "number" && Number.isFinite(tone_squelch_hz)
        ? tone_squelch_hz
        : null,
    offsetHz: typeof offset_hz === "number" ? offset_hz : 0,
    status: status === "paused" ? "paused" : "active",
  };
}

export function parseVfoList(raw: unknown): Vfo[] {
  if (!Array.isArray(raw)) return bad("vfos is not a list");
  return raw.map(parseVfo).sort((a, b) => a.id - b.id);
}

/** `{ "v":1, "vfos":[...] }` as returned by GET /vfos. */
export function parseListReply(raw: unknown): Vfo[] {
  if (!isRec(raw)) return bad("not an object");
  return parseVfoList(raw.vfos ?? []);
}

function tryParseState(raw: unknown): DeviceState | null {
  if (!isRec(raw)) return null;
  try {
    return parseDeviceState(raw);
  } catch {
    return null;
  }
}

/** `{ "ok":true, "vfo":{...}, "state":{...} }` as returned by POST/PATCH. */
export function parseVfoReply(raw: unknown, status?: number): VfoReply {
  if (!isRec(raw)) throw new ApiError("invalid reply", status);
  if (raw.ok === false) {
    const msg = typeof raw.error === "string" ? raw.error : "request rejected";
    throw new ApiError(
      msg,
      status,
      typeof raw.code === "string" ? raw.code : undefined,
      parseHolders(raw.holders),
    );
  }
  if (raw.ok !== true) throw new ApiError("invalid reply", status);
  const vfo = parseVfo(raw.vfo);
  const rawState = raw.state;
  const vfos = isRec(rawState) && Array.isArray(rawState.vfos) ? parseVfoList(rawState.vfos) : null;
  const state = tryParseState(rawState);
  const r = raw.retune;
  const retune: Retune = r === "applied" || r === "locked" || r === "failed" ? r : "none";
  return { vfo, vfos, state, retune };
}

export function createBody(req: VfoCreate): Rec {
  const body: Rec = { v: 1, freq_hz: req.freqHz, mode: req.mode };
  if (req.bandwidthHz !== undefined) body.bandwidth_hz = req.bandwidthHz;
  if (req.squelchDbfs !== undefined) body.squelch_dbfs = req.squelchDbfs;
  if (req.cwPitchHz !== undefined) body.cw_pitch_hz = req.cwPitchHz;
  if (req.toneSquelchHz !== undefined) body.tone_squelch_hz = req.toneSquelchHz;
  return body;
}

export function patchBody(patch: VfoPatch): Rec {
  const body: Rec = { v: 1 };
  if (patch.freqHz !== undefined) body.freq_hz = patch.freqHz;
  if (patch.mode !== undefined) body.mode = patch.mode;
  if (patch.bandwidthHz !== undefined) body.bandwidth_hz = patch.bandwidthHz;
  if (patch.squelchDbfs !== undefined) body.squelch_dbfs = patch.squelchDbfs;
  if (patch.cwPitchHz !== undefined) body.cw_pitch_hz = patch.cwPitchHz;
  if (patch.stereo !== undefined) body.stereo = patch.stereo;
  if (patch.tones !== undefined) body.tones = patch.tones;
  if (patch.toneSquelchHz !== undefined) body.tone_squelch_hz = patch.toneSquelchHz;
  return body;
}

export function createVfoApi(fetchImpl: typeof fetch = apiFetch): VfoApi {
  async function call(url: string, init: RequestInit): Promise<{ res: Response; body: unknown }> {
    let res: Response;
    try {
      res = await fetchImpl(url, init);
    } catch (e) {
      throw new ApiError(e instanceof Error ? e.message : "network error");
    }
    const body = res.status === 204 ? undefined : await readJson(res);
    if (!res.ok) throw httpError(res, body);
    return { res, body };
  }
  const json = (method: string, body: Rec): RequestInit => ({
    method,
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });

  return {
    async list(device, signal) {
      const { body } = await call(`${deviceUrl(device)}/vfos`, { signal });
      return parseListReply(body);
    },
    async create(device, req) {
      const { res, body } = await call(`${deviceUrl(device)}/vfos`, json("POST", createBody(req)));
      return parseVfoReply(body, res.status);
    },
    async update(device, id, patch) {
      const { res, body } = await call(
        `${deviceUrl(device)}/vfos/${id}`,
        json("PATCH", patchBody(patch)),
      );
      return parseVfoReply(body, res.status);
    },
    async remove(device, id) {
      await call(`${deviceUrl(device)}/vfos/${id}`, { method: "DELETE" });
    },
  };
}
