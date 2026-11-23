// Typed client for the ADS-B routes (docs/api-plugins.md, docs/plugins-adsb.md):
//   GET    /api/v1/plugin-instances/{id}/status -> AdsbReply (everybody who sees decoders)
//   GET    /api/v1/adsb/lookup                  -> registration, type, airline (local database)
//   GET    /api/v1/adsb/range                   -> range statistics of a device
//   DELETE /api/v1/adsb/range                   -> reset them (operator)
//   GET    /api/v1/adsb/aircraft-db             -> source, licence and import progress
//   POST / DELETE /api/v1/admin/adsb/aircraft-db -> import / remove the database (admin)
//   GET    /api/v1/adsb/history                 -> "seen before" statistics of one aircraft (local)
//   GET    /api/v1/adsb/online                  -> online details of one aircraft (opt-in, ADR-0033)
//   GET / PUT /api/v1/admin/adsb/online         -> the admin's switches for the online sources

import { apiFetch } from "../auth/session";
import { ApiError, httpError, readJson } from "../spectrum/api";
import { parseDatabaseStatus, parseLookup, type DatabaseStatus, type LookupReply } from "./info";
import { parseReply, type AdsbReply } from "./model";
import {
  parseHistory,
  parseOnline,
  parseOnlineAdmin,
  type OnlineAdmin,
  type OnlineReply,
  type OnlineSettings,
  type SeenHistory,
} from "./online";
import { parseRange, type RangeData } from "./range";

export interface AdsbApi {
  status(instanceId: string, signal?: AbortSignal): Promise<AdsbReply>;
  lookup(icao: readonly string[], operators: readonly string[]): Promise<LookupReply>;
  range(device: string, hours: number, signal?: AbortSignal): Promise<RangeData>;
  resetRange(device: string): Promise<void>;
  database(signal?: AbortSignal): Promise<DatabaseStatus>;
  updateDatabase(source: "download" | "file"): Promise<void>;
  removeDatabase(): Promise<void>;
  /** Details from the enabled online sources; nothing is fetched while they are off. */
  online(icao: string, callsign: string | null, signal?: AbortSignal): Promise<OnlineReply>;
  /** When the decoder history first and last heard the aircraft; null if never. */
  history(icao: string, signal?: AbortSignal): Promise<SeenHistory | null>;
  onlineSettings(signal?: AbortSignal): Promise<OnlineAdmin>;
  setOnlineSettings(settings: OnlineSettings): Promise<void>;
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
    async online(icao, callsign, signal) {
      const q = new URLSearchParams({ icao });
      if (callsign !== null && callsign !== "") q.set("callsign", callsign);
      const data = parseOnline(await call(`/api/v1/adsb/online?${q.toString()}`, { signal }));
      if (data === null) throw new ApiError("invalid online details reply");
      return data;
    },
    async history(icao, signal) {
      const q = new URLSearchParams({ icao });
      return parseHistory(await call(`/api/v1/adsb/history?${q.toString()}`, { signal }));
    },
    async onlineSettings(signal) {
      const data = parseOnlineAdmin(await call("/api/v1/admin/adsb/online", { signal }));
      if (data === null) throw new ApiError("invalid online settings reply");
      return data;
    },
    async setOnlineSettings(settings) {
      await call("/api/v1/admin/adsb/online", {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(settings),
      });
    },
  };
}
