// REST client of the weather routes (docs/api-weather.md).

import { apiFetch } from "../auth/session";
import {
  parseCapture,
  parseConfig,
  parseElements,
  parsePasses,
  type Capture,
  type ElementsInfo,
  type PassesReply,
  type WeatherConfig,
} from "./model";

export class WeatherError extends Error {
  constructor(
    message: string,
    readonly status: number,
    readonly code: string,
  ) {
    super(message);
    this.name = "WeatherError";
  }
}

type Rec = Record<string, unknown>;
const isRec = (v: unknown): v is Rec => typeof v === "object" && v !== null && !Array.isArray(v);
const str = (v: unknown): string | null => (typeof v === "string" ? v : null);

const TEXT: Record<string, string> = {
  no_elements:
    "There are no orbit data yet. An admin can allow the download below or paste the data.",
  no_position: "The receiver position is not known. Enter it below (ADS-B uses the same fields).",
  download_off: "The download of orbit data is switched off.",
  too_soon: "CelesTrak asks for at most one download every 2 hours.",
  device_forbidden: "This device is not open to your account.",
  device_read_only: "This device is read-only for your account.",
};

async function call(
  fetchImpl: typeof fetch,
  method: string,
  url: string,
  body?: unknown,
): Promise<unknown> {
  let res: Response;
  try {
    res = await fetchImpl(url, {
      method,
      headers: body === undefined ? undefined : { "Content-Type": "application/json" },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
  } catch {
    throw new WeatherError("Cannot reach the server.", 0, "network");
  }
  let parsed: unknown;
  try {
    parsed = await res.json();
  } catch {
    parsed = undefined;
  }
  if (res.ok) return parsed;
  const rec = isRec(parsed) ? parsed : {};
  const code = str(rec.code) ?? str(rec.error) ?? "";
  const text =
    TEXT[code] ??
    (str(rec.code) !== null ? str(rec.error) : null) ??
    (res.status === 403 ? "Your role is not allowed to do that." : null) ??
    `Request failed (HTTP ${res.status}).`;
  throw new WeatherError(text, res.status, code);
}

export interface PassesRequest {
  dev: string;
  lat?: number;
  lon?: number;
  hours?: number;
  minElevationDeg?: number;
}

export interface WeatherApi {
  config(): Promise<WeatherConfig>;
  passes(request: PassesRequest): Promise<PassesReply>;
  captures(query?: {
    dev?: string;
    limit?: number;
    beforeMs?: number;
  }): Promise<{ captures: Capture[]; nextBeforeMs: number | null }>;
  keep(id: string, keep: boolean): Promise<Capture>;
  remove(id: string): Promise<void>;
  setDownload(enabled: boolean): Promise<ElementsInfo>;
  refresh(): Promise<ElementsInfo>;
  paste(text: string): Promise<ElementsInfo>;
}

export function createWeatherApi(fetchImpl: typeof fetch = apiFetch): WeatherApi {
  const elements = (body: unknown): ElementsInfo =>
    parseElements(isRec(body) ? body.elements : undefined);
  return {
    async config() {
      return parseConfig(await call(fetchImpl, "GET", "/api/v1/weather/config"));
    },
    async passes(request) {
      const body: Rec = { dev: request.dev };
      if (request.lat !== undefined && request.lon !== undefined) {
        body.lat = request.lat;
        body.lon = request.lon;
      }
      if (request.hours !== undefined) body.hours = request.hours;
      if (request.minElevationDeg !== undefined) body.min_elevation_deg = request.minElevationDeg;
      return parsePasses(await call(fetchImpl, "POST", "/api/v1/weather/passes", body));
    },
    async captures(query = {}) {
      const q = new URLSearchParams();
      if (query.dev !== undefined) q.set("dev", query.dev);
      if (query.limit !== undefined) q.set("limit", String(query.limit));
      if (query.beforeMs !== undefined) q.set("before_ms", String(query.beforeMs));
      const qs = q.toString();
      const body = await call(
        fetchImpl,
        "GET",
        `/api/v1/weather/captures${qs === "" ? "" : `?${qs}`}`,
      );
      const rec = isRec(body) ? body : {};
      const next = typeof rec.next_before_ms === "number" ? rec.next_before_ms : null;
      return {
        captures: (Array.isArray(rec.captures) ? rec.captures : [])
          .map(parseCapture)
          .filter((c): c is Capture => c !== null),
        nextBeforeMs: next,
      };
    },
    async keep(id, keep) {
      const body = await call(
        fetchImpl,
        "PATCH",
        `/api/v1/weather/captures/${encodeURIComponent(id)}`,
        { keep },
      );
      const parsed = parseCapture(body);
      if (parsed === null) throw new WeatherError("Unexpected reply.", 200, "bad_reply");
      return parsed;
    },
    async remove(id) {
      await call(fetchImpl, "DELETE", `/api/v1/weather/captures/${encodeURIComponent(id)}`);
    },
    async setDownload(enabled) {
      return elements(await call(fetchImpl, "PUT", "/api/v1/admin/weather/elements", { enabled }));
    },
    async refresh() {
      return elements(await call(fetchImpl, "POST", "/api/v1/admin/weather/elements/refresh"));
    },
    async paste(text) {
      return elements(
        await call(fetchImpl, "POST", "/api/v1/admin/weather/elements/manual", { text }),
      );
    },
  };
}

export const weatherApi: WeatherApi = createWeatherApi();
