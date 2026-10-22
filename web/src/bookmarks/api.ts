// Typed wrappers for the bookmark REST routes with runtime validation.
//   GET /api/v1/bookmarks            -> { v, bookmarks: [...] }
//   POST /api/v1/bookmarks           -> 201 bookmark
//   PATCH /api/v1/bookmarks/{id}     -> bookmark (partial update)
//   DELETE /api/v1/bookmarks/{id}    -> 204
//   POST /api/v1/bookmarks/bulk      -> { v, created }   body { bookmarks, replace }
// Errors are { v, ok:false, code, error }.

import { apiFetch } from "../auth/session";
import { ApiError, httpError, isRec, readJson, type Rec } from "../spectrum/api";
import { VFO_MODES, type VfoMode } from "../vfo/api";
import type { Bookmark, BookmarkInput } from "./model";

export interface BookmarkApi {
  list(signal?: AbortSignal): Promise<Bookmark[]>;
  create(input: BookmarkInput): Promise<void>;
  update(id: string, patch: Partial<BookmarkInput>): Promise<void>;
  remove(id: string): Promise<void>;
  /** Create many at once; `replace` deletes all existing bookmarks first. Resolves with the count. */
  bulk(items: readonly BookmarkInput[], replace: boolean): Promise<number>;
}

function bad(what: string): never {
  throw new ApiError(`invalid bookmark: ${what}`);
}

const finite = (v: unknown): v is number => typeof v === "number" && Number.isFinite(v);

export function parseBookmark(raw: unknown): Bookmark {
  if (!isRec(raw)) return bad("not an object");
  const { id, name, freq_hz, mode, bandwidth_hz, tags, color, notes, created_ms, updated_ms } = raw;
  if (typeof id !== "string" && !(typeof id === "number" && Number.isFinite(id))) return bad("id");
  if (typeof name !== "string") return bad("name");
  if (!finite(freq_hz) || freq_hz <= 0) return bad("freq_hz");
  const vfoMode: VfoMode | null = VFO_MODES.find((m) => m === mode) ?? null;
  return {
    id: String(id),
    name,
    freqHz: freq_hz,
    mode: vfoMode,
    bandwidthHz: finite(bandwidth_hz) && bandwidth_hz > 0 ? bandwidth_hz : null,
    tags: Array.isArray(tags) ? tags.filter((t): t is string => typeof t === "string") : [],
    color: typeof color === "string" && color !== "" ? color : null,
    notes: typeof notes === "string" && notes !== "" ? notes : null,
    createdMs: finite(created_ms) ? created_ms : 0,
    updatedMs: finite(updated_ms) ? updated_ms : 0,
  };
}

export function parseBookmarkList(raw: unknown): Bookmark[] {
  const list = isRec(raw) ? raw.bookmarks : undefined;
  if (!Array.isArray(list)) return bad("list");
  return list.map(parseBookmark);
}

/** Wire form of an input (snake_case; nulls kept so a PATCH can clear a field). */
export function inputBody(input: Partial<BookmarkInput>): Rec {
  const body: Rec = {};
  if (input.name !== undefined) body.name = input.name;
  if (input.freqHz !== undefined) body.freq_hz = Math.round(input.freqHz);
  if (input.mode !== undefined) body.mode = input.mode;
  if (input.bandwidthHz !== undefined) body.bandwidth_hz = input.bandwidthHz;
  if (input.tags !== undefined) body.tags = input.tags;
  if (input.color !== undefined) body.color = input.color;
  if (input.notes !== undefined) body.notes = input.notes;
  return body;
}

export function createBookmarkApi(fetchImpl: typeof fetch = apiFetch): BookmarkApi {
  async function call(url: string, init?: RequestInit): Promise<unknown> {
    let res: Response;
    try {
      res = await fetchImpl(url, init);
    } catch (e) {
      throw new ApiError(e instanceof Error ? e.message : "network error");
    }
    const body = res.status === 204 ? undefined : await readJson(res);
    if (!res.ok) throw httpError(res, body);
    return body;
  }
  const json = (method: string, body: Rec): RequestInit => ({
    method,
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ v: 1, ...body }),
  });
  const url = (id?: string) =>
    `/api/v1/bookmarks${id === undefined ? "" : `/${encodeURIComponent(id)}`}`;

  return {
    async list(signal) {
      return parseBookmarkList(await call(url(), { signal }));
    },
    async create(input) {
      await call(url(), json("POST", inputBody(input)));
    },
    async update(id, patch) {
      await call(url(id), json("PATCH", inputBody(patch)));
    },
    async remove(id) {
      await call(url(id), { method: "DELETE" });
    },
    async bulk(items, replace) {
      const body = await call(
        "/api/v1/bookmarks/bulk",
        json("POST", { bookmarks: items.map(inputBody), replace }),
      );
      return isRec(body) && finite(body.created) ? body.created : items.length;
    },
  };
}
