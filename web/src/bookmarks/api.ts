// Typed wrappers for the bookmark REST routes with runtime validation.
//   GET /api/v1/bookmarks            -> { v, bookmarks: [...] }
//   POST /api/v1/bookmarks           -> 201 bookmark
//   PATCH /api/v1/bookmarks/{id}     -> bookmark (partial update)
//   DELETE /api/v1/bookmarks/{id}    -> 204
//   POST /api/v1/bookmarks/bulk      -> { v, created }   body { bookmarks, replace }
//   GET/POST /api/v1/bookmark-groups, PATCH/DELETE /api/v1/bookmark-groups/{id},
//   PUT /api/v1/bookmark-groups/order
// Errors are { v, ok:false, code, error }. Contract: docs/api-bookmarks.md.

import { apiFetch } from "../auth/session";
import { ApiError, httpError, isRec, readJson, type Rec } from "../spectrum/api";
import { VFO_MODES, type VfoMode } from "../vfo/api";
import {
  DIGITAL_PROTOCOLS,
  NO_SCAN,
  type Bookmark,
  type BookmarkGroup,
  type BookmarkInput,
  type DcsPolarity,
  type DigitalChannel,
  type ScanSettings,
  type ToneMode,
} from "./model";

export interface BookmarkApi {
  list(signal?: AbortSignal): Promise<Bookmark[]>;
  create(input: BookmarkInput): Promise<void>;
  update(id: string, patch: Partial<BookmarkInput>): Promise<void>;
  remove(id: string): Promise<void>;
  /** Create many at once; `replace` deletes all existing bookmarks first. Resolves with the count. */
  bulk(items: readonly BookmarkInput[], replace: boolean): Promise<number>;
  listGroups(signal?: AbortSignal): Promise<BookmarkGroup[]>;
  createGroup(name: string): Promise<BookmarkGroup>;
  renameGroup(id: string, name: string): Promise<void>;
  removeGroup(id: string): Promise<void>;
  /** Put the listed groups first, in this order. */
  orderGroups(ids: readonly string[]): Promise<void>;
}

function bad(what: string): never {
  throw new ApiError(`invalid bookmark: ${what}`);
}

const finite = (v: unknown): v is number => typeof v === "number" && Number.isFinite(v);
const num = (v: unknown): number | null => (finite(v) ? v : null);

function parseDigital(raw: unknown): DigitalChannel | null {
  if (!isRec(raw)) return null;
  const protocol = DIGITAL_PROTOCOLS.find((p) => p === raw.protocol);
  if (protocol === undefined) return null;
  return {
    protocol,
    timeslot: num(raw.timeslot),
    talkgroup: num(raw.talkgroup),
    colourCode: num(raw.colour_code),
    mcc: num(raw.mcc),
    mnc: num(raw.mnc),
    decoderPluginId:
      typeof raw.decoder_plugin_id === "string" && raw.decoder_plugin_id !== ""
        ? raw.decoder_plugin_id
        : null,
  };
}

function parseScan(raw: unknown): ScanSettings {
  if (!isRec(raw)) return NO_SCAN;
  return { include: raw.include === true, priority: num(raw.priority), dwellMs: num(raw.dwell_ms) };
}

export function parseBookmark(raw: unknown): Bookmark {
  if (!isRec(raw)) return bad("not an object");
  const { id, name, freq_hz, mode, bandwidth_hz, tags, color, notes, created_ms, updated_ms } = raw;
  if (typeof id !== "string" && !(typeof id === "number" && Number.isFinite(id))) return bad("id");
  if (typeof name !== "string") return bad("name");
  if (!finite(freq_hz) || freq_hz <= 0) return bad("freq_hz");
  const vfoMode: VfoMode | null = VFO_MODES.find((m) => m === mode) ?? null;
  const toneMode: ToneMode | null =
    raw.tone_mode === "none" || raw.tone_mode === "ctcss" || raw.tone_mode === "dcs"
      ? raw.tone_mode
      : null;
  const polarity: DcsPolarity | null =
    raw.dcs_polarity === "N" || raw.dcs_polarity === "R" ? raw.dcs_polarity : null;
  return {
    id: String(id),
    name,
    freqHz: freq_hz,
    mode: vfoMode,
    bandwidthHz: finite(bandwidth_hz) && bandwidth_hz > 0 ? bandwidth_hz : null,
    tags: Array.isArray(tags) ? tags.filter((t): t is string => typeof t === "string") : [],
    color: typeof color === "string" && color !== "" ? color : null,
    notes: typeof notes === "string" && notes !== "" ? notes : null,
    squelchDb: num(raw.squelch_db),
    squelchAuto: raw.squelch_auto === true,
    stepHz: finite(raw.step_hz) && raw.step_hz > 0 ? raw.step_hz : null,
    toneMode,
    ctcssHz: num(raw.ctcss_hz),
    dcsCode: num(raw.dcs_code),
    dcsPolarity: polarity,
    digital: parseDigital(raw.digital),
    groupId: typeof raw.group_id === "string" && raw.group_id !== "" ? raw.group_id : null,
    favourite: raw.favourite === true,
    scan: parseScan(raw.scan),
    createdMs: finite(created_ms) ? created_ms : 0,
    updatedMs: finite(updated_ms) ? updated_ms : 0,
  };
}

export function parseBookmarkList(raw: unknown): Bookmark[] {
  const list = isRec(raw) ? raw.bookmarks : undefined;
  if (!Array.isArray(list)) return bad("list");
  return list.map(parseBookmark);
}

export function parseGroup(raw: unknown): BookmarkGroup {
  if (!isRec(raw)) return bad("group is not an object");
  const { id, name, position } = raw;
  if (typeof id !== "string" || typeof name !== "string") return bad("group");
  return { id, name, position: finite(position) ? position : 0 };
}

export function parseGroupList(raw: unknown): BookmarkGroup[] {
  const list = isRec(raw) ? raw.groups : undefined;
  if (!Array.isArray(list)) return bad("group list");
  return list.map(parseGroup);
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
  if (input.squelchDb !== undefined) body.squelch_db = input.squelchDb;
  if (input.squelchAuto !== undefined) body.squelch_auto = input.squelchAuto;
  if (input.stepHz !== undefined) body.step_hz = input.stepHz;
  if (input.toneMode !== undefined) body.tone_mode = input.toneMode;
  if (input.ctcssHz !== undefined) body.ctcss_hz = input.ctcssHz;
  if (input.dcsCode !== undefined) body.dcs_code = input.dcsCode;
  if (input.dcsPolarity !== undefined) body.dcs_polarity = input.dcsPolarity;
  if (input.digital !== undefined) {
    const d = input.digital;
    body.digital =
      d === null
        ? null
        : {
            protocol: d.protocol,
            timeslot: d.timeslot,
            talkgroup: d.talkgroup,
            colour_code: d.colourCode,
            mcc: d.mcc,
            mnc: d.mnc,
            decoder_plugin_id: d.decoderPluginId,
          };
  }
  if (input.groupId !== undefined) body.group_id = input.groupId;
  if (input.favourite !== undefined) body.favourite = input.favourite;
  if (input.scan !== undefined) {
    body.scan = {
      include: input.scan.include,
      priority: input.scan.priority,
      dwell_ms: input.scan.dwellMs,
    };
  }
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
    body: JSON.stringify(body),
  });
  const url = (id?: string) =>
    `/api/v1/bookmarks${id === undefined ? "" : `/${encodeURIComponent(id)}`}`;
  const groupUrl = (id?: string) =>
    `/api/v1/bookmark-groups${id === undefined ? "" : `/${encodeURIComponent(id)}`}`;

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
    async listGroups(signal) {
      return parseGroupList(await call(groupUrl(), { signal }));
    },
    async createGroup(name) {
      return parseGroup(await call(groupUrl(), json("POST", { name })));
    },
    async renameGroup(id, name) {
      await call(groupUrl(id), json("PATCH", { name }));
    },
    async removeGroup(id) {
      await call(groupUrl(id), { method: "DELETE" });
    },
    async orderGroups(ids) {
      await call(groupUrl("order"), json("PUT", { ids }));
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
