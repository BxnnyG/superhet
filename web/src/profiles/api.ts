// Typed wrappers for the profile routes (docs/api-profiles.md) with runtime validation.

import { apiFetch } from "../auth/session";
import { parseStateDoc, type StateDoc } from "../state/doc";
import {
  ApiError,
  deviceUrl,
  httpError,
  isRec,
  parseHolders,
  readJson,
  type Rec,
} from "../spectrum/api";

export interface Profile {
  id: string;
  name: string;
  createdAt: string;
  updatedAt: string;
  /** Summary fields of the stored document. */
  centerHz: number;
  sampleRate: number;
  vfoCount: number;
  lock: boolean;
  /** The stored document, kept verbatim so a rename does not lose fields. */
  data: Rec;
}

export interface ProfileApi {
  list(signal?: AbortSignal): Promise<Profile[]>;
  create(name: string, data: Rec): Promise<Profile>;
  update(id: string, name: string, data: Rec): Promise<Profile>;
  remove(id: string): Promise<void>;
  /**
   * Snapshot the device (the server asks the daemon) into a profile. A name clash is a 409
   * `conflict` unless `overwrite` is set, which replaces the profile of that name.
   */
  saveCurrent(device: string, name: string, overwrite?: boolean): Promise<Profile>;
  /** Apply a profile atomically; resolves with the device's new state. */
  load(device: string, id: string): Promise<StateDoc>;
}

function bad(what: string): never {
  throw new ApiError(`invalid profile: ${what}`);
}

export function parseProfile(raw: unknown): Profile {
  if (!isRec(raw)) return bad("not an object");
  const { id, name, data, created_at, updated_at } = raw;
  if (typeof id !== "string" || typeof name !== "string") return bad("id/name");
  if (!isRec(data)) return bad("data");
  if (typeof data.center_hz !== "number" || typeof data.sample_rate !== "number") {
    return bad("data.center_hz/sample_rate");
  }
  return {
    id,
    name,
    createdAt: typeof created_at === "string" ? created_at : "",
    updatedAt: typeof updated_at === "string" ? updated_at : "",
    centerHz: data.center_hz,
    sampleRate: data.sample_rate,
    vfoCount: Array.isArray(data.vfos) ? data.vfos.length : 0,
    lock: data.lock === true,
    data,
  };
}

function parseOne(body: unknown): Profile {
  return parseProfile(isRec(body) ? body.profile : undefined);
}

export function createProfileApi(fetchImpl: typeof fetch = apiFetch): ProfileApi {
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
    `/api/v1/profiles${id === undefined ? "" : `/${encodeURIComponent(id)}`}`;

  return {
    async list(signal) {
      const body = await call(url(), { signal });
      const list = isRec(body) ? body.profiles : undefined;
      if (!Array.isArray(list)) return bad("list");
      return list.map(parseProfile);
    },
    async create(name, data) {
      return parseOne(await call(url(), json("POST", { name, data })));
    },
    async update(id, name, data) {
      return parseOne(await call(url(id), json("PUT", { name, data })));
    },
    async remove(id) {
      await call(url(id), { method: "DELETE" });
    },
    async saveCurrent(device, name, overwrite = false) {
      const body: Rec = overwrite ? { name, overwrite: true } : { name };
      return parseOne(await call(`${deviceUrl(device)}/profile/save`, json("POST", body)));
    },
    async load(device, id) {
      const body = await call(`${deviceUrl(device)}/profile/load`, json("POST", { id }));
      if (!isRec(body)) throw new ApiError("invalid reply");
      if (body.ok === false) {
        throw new ApiError(
          typeof body.error === "string" ? body.error : "load rejected",
          undefined,
          typeof body.code === "string" ? body.code : undefined,
          parseHolders(body.holders),
        );
      }
      return parseStateDoc(body.state);
    },
  };
}
