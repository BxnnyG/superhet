// Typed client for the one plugin route the ADS-B view uses (docs/api-plugins.md,
// docs/plugins-adsb.md):
//   GET /api/v1/plugin-instances/{id}/status -> AdsbReply (everybody who sees decoders)

import { apiFetch } from "../auth/session";
import { ApiError, httpError, readJson } from "../spectrum/api";
import { parseReply, type AdsbReply } from "./model";

export interface AdsbApi {
  status(instanceId: string, signal?: AbortSignal): Promise<AdsbReply>;
}

const BASE = "/api/v1/plugin-instances";

export function createAdsbApi(fetchImpl: typeof fetch = apiFetch): AdsbApi {
  return {
    async status(instanceId, signal) {
      let res: Response;
      try {
        res = await fetchImpl(`${BASE}/${encodeURIComponent(instanceId)}/status`, { signal });
      } catch (e) {
        if (e instanceof DOMException && e.name === "AbortError") throw e;
        throw new ApiError(e instanceof Error ? e.message : "network error");
      }
      const parsed = await readJson(res);
      if (!res.ok) throw httpError(res, parsed);
      return parseReply(parsed);
    },
  };
}
