// REST client for the custom channel plans (docs/api-channel-plans.md).
//   GET    /api/v1/channel-plans       -> { v, plans: [{ id, name, doc, created_ms, updated_ms }] }
//   POST   /api/v1/channel-plans       -> 201 plan      body: the plan document
//   PUT    /api/v1/channel-plans/{id}  -> plan          body: the plan document
//   DELETE /api/v1/channel-plans/{id}  -> 204
// Plans that fail the client-side validation are skipped (and counted), never half-loaded.

import { apiFetch } from "../auth/session";
import { ApiError, httpError, isRec, readJson } from "../spectrum/api";
import { parsePlan, type Plan } from "./schema";

export const CUSTOM_PREFIX = "custom:";

export interface CustomPlans {
  plans: Plan[];
  /** Documents the server returned that this version cannot read. */
  skipped: number;
}

export interface ChannelPlanApi {
  list(signal?: AbortSignal): Promise<CustomPlans>;
  create(doc: Record<string, unknown>): Promise<void>;
  update(id: string, doc: Record<string, unknown>): Promise<void>;
  remove(id: string): Promise<void>;
}

/** The server id of a custom plan, or null for a built-in one. */
export function serverId(plan: Plan): string | null {
  return plan.id.startsWith(CUSTOM_PREFIX) ? plan.id.slice(CUSTOM_PREFIX.length) : null;
}

export function parsePlanList(raw: unknown): CustomPlans {
  const list = isRec(raw) ? raw.plans : undefined;
  if (!Array.isArray(list)) throw new ApiError("invalid channel plan list");
  const plans: Plan[] = [];
  let skipped = 0;
  for (const item of list) {
    try {
      if (!isRec(item) || typeof item.id !== "string") throw new Error("no id");
      plans.push(parsePlan(item.doc, { id: `${CUSTOM_PREFIX}${item.id}` }));
    } catch {
      skipped++;
    }
  }
  return { plans, skipped };
}

export function createChannelPlanApi(fetchImpl: typeof fetch = apiFetch): ChannelPlanApi {
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
  const json = (method: string, body: Record<string, unknown>): RequestInit => ({
    method,
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
  const url = (id?: string) =>
    `/api/v1/channel-plans${id === undefined ? "" : `/${encodeURIComponent(id)}`}`;
  return {
    async list(signal) {
      return parsePlanList(await call(url(), { signal }));
    },
    async create(doc) {
      await call(url(), json("POST", doc));
    },
    async update(id, doc) {
      await call(url(id), json("PUT", doc));
    },
    async remove(id) {
      await call(url(id), { method: "DELETE" });
    },
  };
}
