// REST client of the public share API. No cookies are involved: the token in the path is the
// only credential, so these requests never touch the account session helpers.

import { shareApi } from "./paths";

export type ShareFeature = "waterfall" | "audio" | "decoder_events" | "plugin_audio";

export interface ShareInfo {
  title: string;
  description: string;
  location: string;
  device: string;
  features: ShareFeature[];
  tuning:
    | { mode: "follow" }
    | {
        mode: "fixed";
        freqHz: number;
        demod: string;
        range: { minHz: number; maxHz: number } | null;
      };
  listeners: number;
  maxListeners: number;
  expiresAtMs: number | null;
}

export interface ShareVfo {
  id: number;
  freqHz: number;
  mode: string;
}

export interface PluginStream {
  id: string;
  pluginId: string;
  audioVfo: number;
}

export interface ShareState {
  centerHz: number | null;
  sampleRate: number | null;
  vfos: ShareVfo[];
  pluginStreams: PluginStream[];
  listeners: number;
}

/** Why a link cannot be shown. */
export type ShareProblem = "not_found" | "expired" | "revoked" | "busy" | "unavailable";

export type Loaded<T> = { ok: true; value: T } | { ok: false; problem: ShareProblem };

const FEATURES: readonly string[] = ["waterfall", "audio", "decoder_events", "plugin_audio"];

const isObj = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null;
const num = (v: unknown): number | null => (typeof v === "number" && Number.isFinite(v) ? v : null);

export function parseInfo(raw: unknown): ShareInfo | null {
  if (!isObj(raw) || typeof raw.device !== "string") return null;
  const t = isObj(raw.tuning) ? raw.tuning : {};
  const range = isObj(t.range) ? t.range : null;
  const minHz = range ? num(range.min_hz) : null;
  const maxHz = range ? num(range.max_hz) : null;
  const freq = num(t.freq_hz);
  return {
    title: typeof raw.title === "string" ? raw.title : "",
    description: typeof raw.description === "string" ? raw.description : "",
    location: typeof raw.location === "string" ? raw.location : "",
    device: raw.device,
    features: (Array.isArray(raw.features) ? raw.features : []).filter(
      (f): f is ShareFeature => typeof f === "string" && FEATURES.includes(f),
    ),
    tuning:
      t.mode === "fixed" && freq !== null
        ? {
            mode: "fixed",
            freqHz: freq,
            demod: typeof t.demod === "string" ? t.demod : "nfm",
            range: minHz !== null && maxHz !== null ? { minHz, maxHz } : null,
          }
        : { mode: "follow" },
    listeners: num(raw.listeners) ?? 0,
    maxListeners: num(raw.max_listeners) ?? 0,
    expiresAtMs: num(raw.expires_at_ms),
  };
}

export function parseState(raw: unknown): ShareState | null {
  if (!isObj(raw)) return null;
  const vfos = (Array.isArray(raw.vfos) ? raw.vfos : []).flatMap((v): ShareVfo[] => {
    if (!isObj(v)) return [];
    const id = num(v.id);
    const freqHz = num(v.freq_hz);
    return id !== null && freqHz !== null
      ? [{ id, freqHz, mode: typeof v.mode === "string" ? v.mode : "" }]
      : [];
  });
  const pluginStreams = (Array.isArray(raw.plugin_streams) ? raw.plugin_streams : []).flatMap(
    (s): PluginStream[] => {
      if (!isObj(s)) return [];
      const audioVfo = num(s.audio_vfo);
      return audioVfo !== null && typeof s.id === "string"
        ? [{ id: s.id, pluginId: typeof s.plugin_id === "string" ? s.plugin_id : s.id, audioVfo }]
        : [];
    },
  );
  return {
    centerHz: num(raw.center_hz),
    sampleRate: num(raw.sample_rate),
    vfos,
    pluginStreams,
    listeners: num(raw.listeners) ?? 0,
  };
}

async function get<T>(
  url: string,
  parse: (raw: unknown) => T | null,
  fetchImpl: typeof fetch,
  signal?: AbortSignal,
): Promise<Loaded<T>> {
  let r: Response;
  try {
    r = await fetchImpl(url, {
      signal,
      credentials: "omit",
      headers: { accept: "application/json" },
    });
  } catch {
    return { ok: false, problem: "unavailable" };
  }
  if (r.ok) {
    const value = parse(await r.json().catch(() => null));
    return value ? { ok: true, value } : { ok: false, problem: "unavailable" };
  }
  const body: unknown = await r.json().catch(() => null);
  const code = isObj(body) && typeof body.error === "string" ? body.error : "";
  if (r.status === 410) return { ok: false, problem: code === "revoked" ? "revoked" : "expired" };
  if (r.status === 404) return { ok: false, problem: "not_found" };
  if (r.status === 429) return { ok: false, problem: "busy" };
  return { ok: false, problem: "unavailable" };
}

export const fetchInfo = (token: string, signal?: AbortSignal, f: typeof fetch = fetch) =>
  get(shareApi(token).info, parseInfo, f, signal);

export const fetchState = (token: string, signal?: AbortSignal, f: typeof fetch = fetch) =>
  get(shareApi(token).state, parseState, f, signal);

export const PROBLEM_TEXT: Record<ShareProblem, string> = {
  not_found: "This link is not valid.",
  expired: "This link has expired.",
  revoked: "This link was withdrawn by the station.",
  busy: "Too many requests. Please try again in a minute.",
  unavailable: "The receiver is not reachable right now.",
};
