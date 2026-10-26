// Typed wrappers for the decoder event routes (docs/api-decoder-events.md).
//   GET    /api/v1/decoder-events?kind=&dev=&q=&limit=&cursor=  -> { v, events, next_cursor }
//   DELETE /api/v1/decoder-events                                -> { v, deleted }
//   GET    /api/v1/decoder-events/export.csv?kind=&dev=&q=       -> text/csv

import { apiFetch } from "../auth/session";
import { ApiError, httpError, isRec, readJson } from "../spectrum/api";
import { parseEvent, type DecoderEvent, type LogFilter } from "./events";

export interface EventPage {
  events: DecoderEvent[];
  /** Cursor of the next older page; null on the last page. */
  next: string | null;
}

export interface EventApi {
  list(
    device: string,
    filter: LogFilter,
    opts?: { limit?: number; cursor?: string | null; signal?: AbortSignal },
  ): Promise<EventPage>;
  /** Delete the whole history; resolves with the number of removed events. */
  clear(): Promise<number>;
}

const BASE = "/api/v1/decoder-events";

function filterParams(device: string, filter: LogFilter): URLSearchParams {
  const p = new URLSearchParams();
  if (filter.kinds.length > 0) p.set("kind", filter.kinds.join(","));
  p.set("dev", device);
  const q = filter.q.trim();
  if (q !== "") p.set("q", q);
  return p;
}

/** Download URL of the CSV export for the current filter. */
export function exportUrl(device: string, filter: LogFilter): string {
  return `${BASE}/export.csv?${filterParams(device, filter).toString()}`;
}

export function parsePage(raw: unknown): EventPage {
  if (!isRec(raw) || !Array.isArray(raw.events)) throw new ApiError("invalid event list");
  const events: DecoderEvent[] = [];
  for (const item of raw.events) {
    const e = parseEvent(item);
    if (e) events.push(e);
  }
  const next = typeof raw.next_cursor === "string" ? raw.next_cursor : null;
  return { events, next };
}

export function createEventApi(fetchImpl: typeof fetch = apiFetch): EventApi {
  async function call(url: string, init?: RequestInit): Promise<unknown> {
    let res: Response;
    try {
      res = await fetchImpl(url, init);
    } catch (e) {
      if (e instanceof DOMException && e.name === "AbortError") throw e;
      throw new ApiError(e instanceof Error ? e.message : "network error");
    }
    const body = await readJson(res);
    if (!res.ok) throw httpError(res, body);
    return body;
  }
  return {
    async list(device, filter, opts = {}) {
      const p = filterParams(device, filter);
      p.set("limit", String(opts.limit ?? 200));
      if (opts.cursor) p.set("cursor", opts.cursor);
      return parsePage(await call(`${BASE}?${p.toString()}`, { signal: opts.signal }));
    },
    async clear() {
      const body = await call(BASE, { method: "DELETE" });
      return isRec(body) && typeof body.deleted === "number" ? body.deleted : 0;
    },
  };
}
