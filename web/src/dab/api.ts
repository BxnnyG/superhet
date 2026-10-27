// Typed client for the plugin routes the DAB view uses (docs/api-plugins.md, docs/plugins-dab.md).
//   GET  /api/v1/plugin-instances/{id}/status        -> DabReply          (everybody who sees decoders)
//   GET  /api/v1/plugin-instances/{id}/media/{key}   -> image bytes       (same)
//   POST /api/v1/plugin-instances/{id}/control {..}  -> { ok }            (operator)

import { apiFetch } from "../auth/session";
import { httpError, readJson } from "../spectrum/api";
import { ApiError } from "../spectrum/api";
import { parseReply, type DabReply } from "./model";

export type DabControl =
  | { cmd: "tune"; channel: string }
  | { cmd: "select"; service: string | null }
  | { cmd: "scan"; channels?: string[] }
  | { cmd: "scan_stop" };

export interface DabApi {
  status(instanceId: string, signal?: AbortSignal): Promise<DabReply>;
  control(instanceId: string, message: DabControl): Promise<void>;
}

const BASE = "/api/v1/plugin-instances";
const enc = encodeURIComponent;

/** URL of a stored media object; `revision` busts the browser cache when the slide changes. */
export function mediaUrl(instanceId: string, key: string, revision: number): string {
  return `${BASE}/${enc(instanceId)}/media/${enc(key)}?rev=${revision}`;
}

export function createDabApi(fetchImpl: typeof fetch = apiFetch): DabApi {
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
    const parsed = await readJson(res);
    if (!res.ok) throw httpError(res, parsed);
    return parsed;
  }
  return {
    async status(instanceId, signal) {
      return parseReply(await call("GET", `${BASE}/${enc(instanceId)}/status`, undefined, signal));
    },
    async control(instanceId, message) {
      await call("POST", `${BASE}/${enc(instanceId)}/control`, message);
    },
  };
}
