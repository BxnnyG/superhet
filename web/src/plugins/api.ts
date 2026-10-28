// Typed client for the plugin routes (docs/api-plugins.md) with runtime validation.
//   GET    /api/v1/plugins                      -> { plugins: [{ manifest }] }      (operator)
//   POST   /api/v1/plugins { path }             -> { plugin }                       (admin)
//   DELETE /api/v1/plugins/{id}                 -> 204                              (admin)
//   GET    /api/v1/plugin-instances             -> { instances: [InstanceInfo] }    (operator)
//   POST   /api/v1/plugin-instances             -> { instance }                     (operator)
//   POST   /api/v1/plugin-instances/{id}/stop|restart -> { instance }               (operator)
//   POST   /api/v1/plugin-instances/{id}/reconfigure { config } -> { instance }     (operator)
// Errors are `{ ok: false, code, error }`; they surface as ApiError(message, status, code).

import { apiFetch } from "../auth/session";
import { ApiError, httpError, isRec, readJson, type Rec } from "../spectrum/api";

export interface PluginInput {
  kind: "iq" | "discriminator" | "audio" | "voice_frames" | string;
  sampleRate: number;
  format: string;
  /** Channel width the host cuts out (Hz); null when the manifest does not say. */
  bandwidthHz: number | null;
}

export interface PluginDevice {
  exclusive: boolean;
  sampleRate: number | null;
  /** Fixed centre of an exclusive plugin. */
  centerHz: number | null;
  /** Range the instance picks its centre from (`freq_hz` of the start request). */
  centerHzMin: number | null;
  centerHzMax: number | null;
}

export interface PluginAck {
  text: string;
  url: string | null;
}

/** The part of a manifest (`schema/manifest.schema.json`) the UI shows. */
export interface PluginManifest {
  id: string;
  name: string;
  version: string;
  license: string;
  type: "decoder" | "codec" | "lookup" | "ui" | string;
  description: string;
  homepage: string | null;
  transport: string;
  runtimeKind: string;
  input: PluginInput | null;
  device: PluginDevice;
  /** Licence / legal notice the user confirms before the first start. */
  acknowledge: PluginAck | null;
  /** Build commands that the host does not run; shown for information. */
  build: string[];
}

export const INSTANCE_STATES = ["starting", "running", "backoff", "error", "stopped"] as const;
export type InstanceState = (typeof INSTANCE_STATES)[number];

export interface PluginInstance {
  id: string;
  pluginId: string;
  device: string | null;
  vfo: number | null;
  freqHz: number | null;
  config: unknown;
  exclusive: boolean;
  state: InstanceState;
  pid: number | null;
  restarts: number;
  crashesInWindow: number;
  retryInMs: number | null;
  lastError: string | null;
  eventsOk: number;
  eventsInvalid: number;
  inputDropped: number;
  inputError: string | null;
  updatedMs: number;
  /** Sample bytes delivered to the plugin since the instance was created. */
  inputBytes: number;
  /** Smoothed input level in dBFS; null before the first chunk. */
  inputLevelDb: number | null;
  /** Server time (ms) of the last input chunk / the last valid event; null if none yet. */
  lastInputMs: number | null;
  lastEventMs: number | null;
  createdMs: number;
  reconfigures: number;
}

export interface StartRequest {
  pluginId: string;
  device?: string;
  vfo?: number;
  freqHz?: number;
  config?: Rec;
}

export interface PluginApi {
  listPlugins(signal?: AbortSignal): Promise<PluginManifest[]>;
  install(path: string): Promise<PluginManifest>;
  remove(pluginId: string): Promise<void>;
  listInstances(signal?: AbortSignal): Promise<PluginInstance[]>;
  start(req: StartRequest): Promise<PluginInstance>;
  stop(instanceId: string): Promise<PluginInstance>;
  restart(instanceId: string): Promise<PluginInstance>;
  /** Replace the configuration; the process restarts with it (same id, counters kept). */
  reconfigure(instanceId: string, config: Rec): Promise<PluginInstance>;
}

// ---------- parsing ----------

const bad = (what: string): never => {
  throw new ApiError(`invalid ${what} reply`);
};
const str = (v: unknown, d = ""): string => (typeof v === "string" ? v : d);
const strOrNull = (v: unknown): string | null => (typeof v === "string" && v !== "" ? v : null);
const numOrNull = (v: unknown): number | null =>
  typeof v === "number" && Number.isFinite(v) ? v : null;
const count = (v: unknown): number => (typeof v === "number" && Number.isFinite(v) ? v : 0);

export function parseManifest(raw: unknown): PluginManifest {
  if (!isRec(raw) || !isRec(raw.plugin)) return bad("plugin");
  const p = raw.plugin;
  const id = str(p.id);
  if (id === "") return bad("plugin");
  const input = isRec(raw.input) ? raw.input : null;
  const dev = isRec(raw.device) ? raw.device : {};
  const install = isRec(raw.install) ? raw.install : {};
  const ack = isRec(install.acknowledge) ? install.acknowledge : null;
  const runtime = isRec(raw.runtime) ? raw.runtime : {};
  return {
    id,
    name: str(p.name, id),
    version: str(p.version),
    license: str(p.license),
    type: str(p.type, "decoder"),
    description: str(p.description),
    homepage: strOrNull(p.homepage),
    transport: str(runtime.transport, "simple"),
    runtimeKind: str(runtime.kind, "process"),
    input: input
      ? {
          kind: str(input.kind),
          sampleRate: count(input.sample_rate),
          format: str(input.format),
          bandwidthHz: numOrNull(input.bandwidth_hz),
        }
      : null,
    device: {
      exclusive: dev.exclusive === true,
      sampleRate: numOrNull(dev.sample_rate),
      centerHz: numOrNull(dev.center_hz),
      centerHzMin: numOrNull(dev.center_hz_min),
      centerHzMax: numOrNull(dev.center_hz_max),
    },
    acknowledge:
      ack && typeof ack.text === "string" ? { text: ack.text, url: strOrNull(ack.url) } : null,
    build: Array.isArray(install.build) ? install.build.filter((x) => typeof x === "string") : [],
  };
}

export function parsePlugin(raw: unknown): PluginManifest {
  return isRec(raw) ? parseManifest(raw.manifest) : bad("plugin");
}

export function parseInstance(raw: unknown): PluginInstance {
  if (!isRec(raw) || typeof raw.id !== "string" || typeof raw.plugin_id !== "string") {
    return bad("instance");
  }
  const state = INSTANCE_STATES.find((s) => s === raw.state) ?? "error";
  return {
    id: raw.id,
    pluginId: raw.plugin_id,
    device: strOrNull(raw.device),
    vfo: typeof raw.vfo === "number" && Number.isInteger(raw.vfo) ? raw.vfo : null,
    freqHz: numOrNull(raw.freq_hz),
    config: raw.config ?? null,
    exclusive: raw.exclusive === true,
    state,
    pid: numOrNull(raw.pid),
    restarts: count(raw.restarts),
    crashesInWindow: count(raw.crashes_in_window),
    retryInMs: numOrNull(raw.retry_in_ms),
    lastError: strOrNull(raw.last_error),
    eventsOk: count(raw.events_ok),
    eventsInvalid: count(raw.events_invalid),
    inputDropped: count(raw.input_dropped),
    inputError: strOrNull(raw.input_error),
    updatedMs: count(raw.updated_unix_ms),
    inputBytes: count(raw.input_bytes),
    inputLevelDb: numOrNull(raw.input_level_db),
    lastInputMs: numOrNull(raw.last_input_unix_ms),
    lastEventMs: numOrNull(raw.last_event_unix_ms),
    createdMs: count(raw.created_unix_ms),
    reconfigures: count(raw.reconfigures),
  };
}

export function startBody(req: StartRequest): Rec {
  const body: Rec = { plugin_id: req.pluginId };
  if (req.device !== undefined) body.device = req.device;
  if (req.vfo !== undefined) body.vfo = req.vfo;
  if (req.freqHz !== undefined) body.freq_hz = Math.round(req.freqHz);
  if (req.config !== undefined && Object.keys(req.config).length > 0) body.config = req.config;
  return body;
}

// ---------- client ----------

const BASE = "/api/v1";

export function createPluginApi(fetchImpl: typeof fetch = apiFetch): PluginApi {
  async function call(
    method: string,
    url: string,
    body?: unknown,
    signal?: AbortSignal,
  ): Promise<unknown> {
    let res: Response;
    try {
      res = await fetchImpl(url, {
        method,
        headers: body === undefined ? undefined : { "Content-Type": "application/json" },
        body: body === undefined ? undefined : JSON.stringify(body),
        signal,
      });
    } catch (e) {
      if (e instanceof DOMException && e.name === "AbortError") throw e;
      throw new ApiError(e instanceof Error ? e.message : "network error");
    }
    const parsed = res.status === 204 ? undefined : await readJson(res);
    if (!res.ok) throw httpError(res, parsed);
    return parsed;
  }
  const list = (v: unknown): unknown[] => (Array.isArray(v) ? v : []);
  const instance = (raw: unknown) => parseInstance(isRec(raw) ? raw.instance : undefined);
  const enc = encodeURIComponent;
  return {
    async listPlugins(signal) {
      const r = await call("GET", `${BASE}/plugins`, undefined, signal);
      return isRec(r) ? list(r.plugins).map(parsePlugin) : bad("plugin list");
    },
    async install(path) {
      const r = await call("POST", `${BASE}/plugins`, { path });
      return parsePlugin(isRec(r) ? r.plugin : undefined);
    },
    async remove(pluginId) {
      await call("DELETE", `${BASE}/plugins/${enc(pluginId)}`);
    },
    async listInstances(signal) {
      const r = await call("GET", `${BASE}/plugin-instances`, undefined, signal);
      return isRec(r) ? list(r.instances).map(parseInstance) : bad("instance list");
    },
    async start(req) {
      return instance(await call("POST", `${BASE}/plugin-instances`, startBody(req)));
    },
    async stop(instanceId) {
      return instance(await call("POST", `${BASE}/plugin-instances/${enc(instanceId)}/stop`));
    },
    async restart(instanceId) {
      return instance(await call("POST", `${BASE}/plugin-instances/${enc(instanceId)}/restart`));
    },
    async reconfigure(instanceId, config) {
      return instance(
        await call("POST", `${BASE}/plugin-instances/${enc(instanceId)}/reconfigure`, { config }),
      );
    },
  };
}
