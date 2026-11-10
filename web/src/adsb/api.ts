// Typed client for the ADS-B routes (docs/api-plugins.md, docs/plugins-adsb.md):
//   GET    /api/v1/plugin-instances/{id}/status -> AdsbReply (everybody who sees decoders)
//   GET    /api/v1/adsb/lookup                  -> registration, type, airline (local database)
//   GET    /api/v1/adsb/range                   -> range statistics of a device
//   DELETE /api/v1/adsb/range                   -> reset them (operator)
//   GET    /api/v1/adsb/aircraft-db             -> source, licence and import progress
//   POST / DELETE /api/v1/admin/adsb/aircraft-db -> import / remove the database (admin)

import { apiFetch } from "../auth/session";
import { ApiError, httpError, readJson } from "../spectrum/api";
import { parseDatabaseStatus, parseLookup, type DatabaseStatus, type LookupReply } from "./info";
import { parseReply, type AdsbReply } from "./model";
import { parseRange, type RangeData } from "./range";

export interface AdsbApi {
  status(instanceId: string, signal?: AbortSignal): Promise<AdsbReply>;
  lookup(icao: readonly string[], operators: readonly string[]): Promise<LookupReply>;
  range(device: string, hours: number, signal?: AbortSignal): Promise<RangeData>;
  resetRange(device: string): Promise<void>;
  database(signal?: AbortSignal): Promise<DatabaseStatus>;
  updateDatabase(source: "download" | "file"): Promise<void>;
  removeDatabase(): Promise<void>;
}

const BASE = "/api/v1/plugin-instances";

export function createAdsbApi(fetchImpl: typeof fetch = apiFetch): AdsbApi {
  async function call(path: string, init?: RequestInit): Promise<unknown> {
    let res: Response;
    try {
      res = await fetchImpl(path, init);
    } catch (e) {
      if (e instanceof DOMException && e.name === "AbortError") throw e;
      throw new ApiError(e instanceof Error ? e.message : "network error");
    }
    const parsed = await readJson(res);
    if (!res.ok) throw httpError(res, parsed);
    return parsed;
  }
  const post = (body: unknown): RequestInit => ({
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
  return {
    async status(instanceId, signal) {
      return parseReply(await call(`${BASE}/${encodeURIComponent(instanceId)}/status`, { signal }));
    },
    async lookup(icao, operators) {
      const q = new URLSearchParams();
      if (icao.length > 0) q.set("icao", icao.join(","));
      if (operators.length > 0) q.set("operator", operators.join(","));
      return parseLookup(await call(`/api/v1/adsb/lookup?${q.toString()}`));
    },
    async range(device, hours, signal) {
      const q = new URLSearchParams({ device, hours: String(hours) });
      const data = parseRange(await call(`/api/v1/adsb/range?${q.toString()}`, { signal }));
      if (data === null) throw new ApiError("invalid range reply");
      return data;
    },
    async resetRange(device) {
      const q = new URLSearchParams({ device });
      await call(`/api/v1/adsb/range?${q.toString()}`, { method: "DELETE" });
    },
    async database(signal) {
      const data = parseDatabaseStatus(await call("/api/v1/adsb/aircraft-db", { signal }));
      if (data === null) throw new ApiError("invalid aircraft database reply");
      return data;
    },
    async updateDatabase(source) {
      await call("/api/v1/admin/adsb/aircraft-db", post({ source }));
    },
    async removeDatabase() {
      await call("/api/v1/admin/adsb/aircraft-db", { method: "DELETE" });
    },
  };
}
