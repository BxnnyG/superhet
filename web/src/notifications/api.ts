// Typed client for the notification routes (docs/api-notifications.md): rules, targets and the
// delivery log under /api/v1/admin, the own push subscription under /api/v1/me/push.

import { HttpError, isJson, request, type FetchLike, type Json } from "../http";

export type TargetKind = "webhook" | "email" | "push";

export interface Target {
  id: number;
  name: string;
  kind: TargetKind;
  /** Non-secret settings. A webhook shows `url_host` instead of the URL. */
  config: Json;
  /** A secret is stored; its value is never returned. */
  hasSecret: boolean;
}

export type ConditionOp =
  | "equals"
  | "not_equals"
  | "contains"
  | "not_contains"
  | "regex"
  | "gt"
  | "gte"
  | "lt"
  | "lte"
  | "exists";

export interface Condition {
  field: string;
  op: ConditionOp;
  value?: string | number | (string | number)[];
  case_sensitive?: boolean;
}

export interface QuietHours {
  start: string;
  end: string;
  tz: string;
}

export interface Rule {
  id: number;
  name: string;
  enabled: boolean;
  source: string | null;
  kind: string | null;
  conditions: Condition[];
  cooldownSecs: number;
  maxPerHour: number | null;
  quiet: QuietHours | null;
  titleTemplate: string;
  bodyTemplate: string;
  targetIds: number[];
}

/** What the API accepts when creating or replacing a rule. */
export interface RuleBody {
  name: string;
  enabled: boolean;
  source: string | null;
  kind: string | null;
  conditions: Condition[];
  cooldown_secs: number;
  max_per_hour: number | null;
  quiet: QuietHours | null;
  title_template: string;
  body_template: string;
  target_ids: number[];
}

/** What the API accepts for a target; `secret` absent keeps, null removes, text replaces. */
export interface TargetBody {
  name: string;
  kind?: TargetKind;
  config: Json;
  secret?: string | null;
}

export type DeliveryStatus = "pending" | "sent" | "failed";

export interface Delivery {
  id: number;
  ruleName: string;
  targetName: string;
  eventId: number | null;
  status: DeliveryStatus;
  attempts: number;
  lastError: string | null;
  title: string;
  body: string;
  createdAtMs: number;
  finishedAtMs: number | null;
}

export interface DeliveryPage {
  deliveries: Delivery[];
  nextBefore: number | null;
}

export interface PushState {
  /** VAPID public key (base64url) for `pushManager.subscribe`. */
  publicKey: string;
  subscriptions: { id: number; label: string | null; createdAtMs: number }[];
}

export interface PushSubscriptionBody {
  endpoint: string;
  keys: { p256dh: string; auth: string };
  label?: string;
}

export interface NotificationsApi {
  listTargets(signal?: AbortSignal): Promise<Target[]>;
  createTarget(body: TargetBody): Promise<Target>;
  updateTarget(id: number, body: TargetBody): Promise<Target>;
  deleteTarget(id: number): Promise<void>;
  testTarget(id: number): Promise<void>;
  listRules(signal?: AbortSignal): Promise<Rule[]>;
  createRule(body: RuleBody): Promise<Rule>;
  updateRule(id: number, body: RuleBody): Promise<Rule>;
  deleteRule(id: number): Promise<void>;
  testRule(id: number): Promise<void>;
  listDeliveries(
    opts?: { status?: DeliveryStatus; before?: number; limit?: number },
    signal?: AbortSignal,
  ): Promise<DeliveryPage>;
}

export interface PushApi {
  get(signal?: AbortSignal): Promise<PushState>;
  /** Resolves with the id of the stored subscription. */
  subscribe(body: PushSubscriptionBody): Promise<number>;
  unsubscribe(id: number): Promise<void>;
}

function bad(what: string): never {
  throw new HttpError(0, "invalid_reply", `Unexpected reply from the server (${what}).`);
}

const arr = (v: unknown): unknown[] => (Array.isArray(v) ? v : []);
const str = (v: unknown, fallback = ""): string => (typeof v === "string" ? v : fallback);
const num = (v: unknown, fallback = 0): number => (typeof v === "number" ? v : fallback);
const numOrNull = (v: unknown): number | null => (typeof v === "number" ? v : null);
const strOrNull = (v: unknown): string | null => (typeof v === "string" ? v : null);

const KINDS: readonly TargetKind[] = ["webhook", "email", "push"];
const STATUSES: readonly DeliveryStatus[] = ["pending", "sent", "failed"];

export function parseTarget(raw: unknown): Target {
  if (!isJson(raw)) return bad("target");
  const kind = KINDS.find((k) => k === raw.kind);
  if (!kind || typeof raw.id !== "number") return bad("target");
  return {
    id: raw.id,
    name: str(raw.name),
    kind,
    config: isJson(raw.config) ? raw.config : {},
    hasSecret: raw.has_secret === true,
  };
}

export function parseRule(raw: unknown): Rule {
  if (!isJson(raw) || typeof raw.id !== "number") return bad("rule");
  const quiet = isJson(raw.quiet)
    ? { start: str(raw.quiet.start), end: str(raw.quiet.end), tz: str(raw.quiet.tz) }
    : null;
  return {
    id: raw.id,
    name: str(raw.name),
    enabled: raw.enabled !== false,
    source: strOrNull(raw.source),
    kind: strOrNull(raw.kind),
    conditions: arr(raw.conditions).filter(isJson) as unknown as Condition[],
    cooldownSecs: num(raw.cooldown_secs),
    maxPerHour: numOrNull(raw.max_per_hour),
    quiet,
    titleTemplate: str(raw.title_template),
    bodyTemplate: str(raw.body_template),
    targetIds: arr(raw.target_ids).filter((v): v is number => typeof v === "number"),
  };
}

export function parseDelivery(raw: unknown): Delivery {
  if (!isJson(raw) || typeof raw.id !== "number") return bad("delivery");
  return {
    id: raw.id,
    ruleName: str(raw.rule_name),
    targetName: str(raw.target_name),
    eventId: numOrNull(raw.event_id),
    status: STATUSES.find((s) => s === raw.status) ?? "pending",
    attempts: num(raw.attempts),
    lastError: strOrNull(raw.last_error),
    title: str(raw.title),
    body: str(raw.body),
    createdAtMs: num(raw.created_at_ms),
    finishedAtMs: numOrNull(raw.finished_at_ms),
  };
}

const ADMIN = "/api/v1/admin";

export function createNotificationsApi(fetchImpl?: FetchLike): NotificationsApi {
  const call = (method: string, url: string, body?: unknown, signal?: AbortSignal) =>
    request(method, url, { body, signal, fetchImpl });
  return {
    async listTargets(signal) {
      const r = await call("GET", `${ADMIN}/notification-targets`, undefined, signal);
      return isJson(r.body) ? arr(r.body.targets).map(parseTarget) : bad("targets");
    },
    async createTarget(body) {
      const r = await call("POST", `${ADMIN}/notification-targets`, body);
      return isJson(r.body) ? parseTarget(r.body.target) : bad("target");
    },
    async updateTarget(id, body) {
      const r = await call("PUT", `${ADMIN}/notification-targets/${id}`, body);
      return isJson(r.body) ? parseTarget(r.body.target) : bad("target");
    },
    async deleteTarget(id) {
      await call("DELETE", `${ADMIN}/notification-targets/${id}`);
    },
    async testTarget(id) {
      await call("POST", `${ADMIN}/notification-targets/${id}/test`);
    },
    async listRules(signal) {
      const r = await call("GET", `${ADMIN}/notification-rules`, undefined, signal);
      return isJson(r.body) ? arr(r.body.rules).map(parseRule) : bad("rules");
    },
    async createRule(body) {
      const r = await call("POST", `${ADMIN}/notification-rules`, body);
      return isJson(r.body) ? parseRule(r.body.rule) : bad("rule");
    },
    async updateRule(id, body) {
      const r = await call("PUT", `${ADMIN}/notification-rules/${id}`, body);
      return isJson(r.body) ? parseRule(r.body.rule) : bad("rule");
    },
    async deleteRule(id) {
      await call("DELETE", `${ADMIN}/notification-rules/${id}`);
    },
    async testRule(id) {
      await call("POST", `${ADMIN}/notification-rules/${id}/test`);
    },
    async listDeliveries(opts = {}, signal) {
      const q = new URLSearchParams();
      if (opts.status) q.set("status", opts.status);
      if (opts.before !== undefined) q.set("before", String(opts.before));
      q.set("limit", String(opts.limit ?? 50));
      const r = await call("GET", `${ADMIN}/notification-deliveries?${q}`, undefined, signal);
      if (!isJson(r.body)) return bad("deliveries");
      return {
        deliveries: arr(r.body.deliveries).map(parseDelivery),
        nextBefore: numOrNull(r.body.next_before),
      };
    },
  };
}

export function createPushApi(fetchImpl?: FetchLike): PushApi {
  const call = (method: string, url: string, body?: unknown, signal?: AbortSignal) =>
    request(method, url, { body, signal, fetchImpl });
  return {
    async get(signal) {
      const r = await call("GET", "/api/v1/me/push", undefined, signal);
      if (!isJson(r.body) || typeof r.body.public_key !== "string") return bad("push state");
      return {
        publicKey: r.body.public_key,
        subscriptions: arr(r.body.subscriptions)
          .filter(isJson)
          .map((s) => ({
            id: num(s.id),
            label: strOrNull(s.label),
            createdAtMs: num(s.created_at_ms),
          })),
      };
    },
    async subscribe(body) {
      const r = await call("POST", "/api/v1/me/push/subscriptions", body);
      const sub = isJson(r.body) && isJson(r.body.subscription) ? r.body.subscription : null;
      return sub && typeof sub.id === "number" ? sub.id : bad("subscription");
    },
    async unsubscribe(id) {
      await call("DELETE", `/api/v1/me/push/subscriptions/${id}`);
    },
  };
}
