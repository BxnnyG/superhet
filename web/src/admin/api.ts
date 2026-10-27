// Typed client for /api/v1/admin/* and /api/v1/me/sessions (docs/api-users.md, docs/api-audit.md)
// with runtime validation of every reply.

import type { Role } from "../auth/api";
import { isRole } from "../auth/api";
import { request, isJson, HttpError, type FetchLike, type Json } from "../http";
import { parseClientInfo, type ClientInfo } from "./clientInfo";
import { parseReportedClient, type ReportedClient } from "./clientReport";

// ---------- models ----------

export interface User {
  id: number;
  username: string;
  role: Role;
  disabled: boolean;
  mustChangePassword: boolean;
  createdAtMs: number;
  lastLoginMs: number | null;
  /** Live sessions of the account. */
  sessions: number;
}

export type InviteState = "open" | "used" | "expired" | "revoked";

export interface Invite {
  id: number;
  role: Role;
  note: string;
  createdBy: string;
  createdAtMs: number;
  expiresAtMs: number;
  state: InviteState;
  usedBy: string | null;
}

export interface CreatedInvite {
  invite: Invite;
  /** The secret token; shown once. */
  token: string;
  /** `/invite/<token>`; prefix it with the origin to get the link. */
  path: string;
}

export interface SessionConnection {
  kind: string;
  device: string;
  sinceMs: number;
}

export interface SessionVfo {
  device: string;
  id: number;
  freqHz: number;
  mode: string;
  status: string;
}

export interface LiveSession {
  /** `u.<hex>` for accounts, `g.<id>` for guests. */
  sid: string;
  kind: "user" | "guest";
  userId: number | null;
  username: string | null;
  role: Role;
  /** Client address (behind a trusted proxy the forwarded one, else the peer). */
  ip: string | null;
  userAgent: string | null;
  /** Peer address, address type, reverse DNS, GeoIP and parsed browser. */
  client: ClientInfo;
  /** What the browser reported about itself (unverified); null when nothing was reported. */
  reported: ReportedClient | null;
  createdAtMs: number;
  lastSeenMs: number | null;
  /** Guests only: seconds without a request. */
  idleSecs: number | null;
  connections: SessionConnection[];
  vfos: SessionVfo[];
}

export interface MySession {
  sid: string;
  ip: string | null;
  userAgent: string | null;
  client: ClientInfo;
  createdAtMs: number;
  lastSeenMs: number;
  current: boolean;
}

export type BanState = "active" | "expired" | "lifted";

export interface Ban {
  id: number;
  /** CIDR text, e.g. `203.0.113.7/32`. */
  network: string;
  reason: string | null;
  state: BanState;
  createdAtMs: number;
  createdBy: string;
  /** Deadline; null = permanent. */
  expiresAtMs: number | null;
  liftedAtMs: number | null;
  liftedBy: string | null;
}

/** What to ban: a network typed by the admin, or a live session (its exact address). */
export interface BanRequest {
  target: { network: string } | { session: string };
  reason?: string;
  /** Absent = permanent. */
  expiresInMinutes?: number;
  /** End matching sessions at once (server default: true). */
  kick?: boolean;
}

export type Outcome = "ok" | "denied" | "error";
export type ActorKind = "user" | "guest" | "system";

export interface AuditEvent {
  id: number;
  tsMs: number;
  actorKind: ActorKind;
  actorId: string | null;
  actorName: string | null;
  role: Role | null;
  ip: string | null;
  userAgent: string | null;
  client: ClientInfo;
  /** `session.client_report` events: what the browser reported (unverified). */
  reported: ReportedClient | null;
  action: string;
  target: string | null;
  outcome: Outcome;
  details: Json;
}

/** Filters of `GET /api/v1/admin/audit` (all optional). */
export interface AuditFilter {
  fromMs?: number;
  toMs?: number;
  actorKind?: ActorKind;
  actor?: string;
  role?: Role;
  action?: string;
  outcome?: Outcome;
  ip?: string;
  target?: string;
}

export interface AuditPage {
  events: AuditEvent[];
  /** Cursor for the next (older) page; null on the last page. */
  nextBeforeId: number | null;
}

export type ExportFormat = "csv" | "json";

export interface GuestPolicy {
  enabled: boolean;
  maxGuests: number;
  maxGuestsPerIp: number;
  maxVfosPerGuest: number;
  allowedModes: string[];
  freqMinHz: number | null;
  freqMaxHz: number | null;
  idleTimeoutSecs: number;
  onlyWithoutLock: boolean;
  showDecoderEvents: boolean;
  privacyText: string;
}

export type IpMode = "full" | "truncated" | "none";

export interface AuditSettings {
  retentionUserDays: number;
  retentionGuestDays: number;
  ipModeUser: IpMode;
  ipModeGuest: IpMode;
}

export interface AuditStats {
  count: number;
  oldestMs: number | null;
  bytes: number;
}

export interface Limits {
  listenerMaxVfos: number;
  operatorMaxVfos: number;
}

// ---------- parsing ----------

function bad(what: string): never {
  throw new HttpError(0, "invalid_reply", `Unexpected reply from the server (${what}).`);
}

const str = (v: unknown, fallback = ""): string => (typeof v === "string" ? v : fallback);
const strOrNull = (v: unknown): string | null => (typeof v === "string" ? v : null);
const num = (v: unknown, fallback = 0): number =>
  typeof v === "number" && Number.isFinite(v) ? v : fallback;
const numOrNull = (v: unknown): number | null =>
  typeof v === "number" && Number.isFinite(v) ? v : null;
const list = (v: unknown): unknown[] => (Array.isArray(v) ? v : []);

export function parseUser(raw: unknown): User {
  if (!isJson(raw) || typeof raw.id !== "number" || !isRole(raw.role)) return bad("user");
  return {
    id: raw.id,
    username: str(raw.username),
    role: raw.role,
    disabled: raw.disabled === true,
    mustChangePassword: raw.must_change_password === true,
    createdAtMs: num(raw.created_at_ms),
    lastLoginMs: numOrNull(raw.last_login_ms),
    sessions: num(raw.sessions),
  };
}

const INVITE_STATES: readonly InviteState[] = ["open", "used", "expired", "revoked"];

export function parseInvite(raw: unknown): Invite {
  if (!isJson(raw) || typeof raw.id !== "number" || !isRole(raw.role)) return bad("invite");
  const state = INVITE_STATES.find((s) => s === raw.state) ?? "expired";
  const who = (v: unknown): string | null =>
    typeof v === "string" ? v : typeof v === "number" ? String(v) : null;
  return {
    id: raw.id,
    role: raw.role,
    note: str(raw.note),
    createdBy: who(raw.created_by) ?? "",
    createdAtMs: num(raw.created_at_ms),
    expiresAtMs: num(raw.expires_at_ms),
    state,
    usedBy: who(raw.used_by),
  };
}

export function parseLiveSession(raw: unknown): LiveSession {
  if (!isJson(raw) || typeof raw.sid !== "string") return bad("session");
  const kind = raw.kind === "guest" ? "guest" : "user";
  return {
    sid: raw.sid,
    kind,
    userId: numOrNull(raw.user_id),
    username: strOrNull(raw.username),
    role: isRole(raw.role) ? raw.role : "listener",
    ip: strOrNull(raw.ip),
    userAgent: strOrNull(raw.user_agent),
    client: parseClientInfo(raw),
    reported: parseReportedClient(raw.client_report),
    createdAtMs: num(raw.created_at_ms),
    lastSeenMs: numOrNull(raw.last_seen_ms),
    idleSecs: numOrNull(raw.idle_secs),
    connections: list(raw.connections).flatMap((c) =>
      isJson(c) ? [{ kind: str(c.kind), device: str(c.device), sinceMs: num(c.since_ms) }] : [],
    ),
    vfos: list(raw.vfos).flatMap((v) =>
      isJson(v)
        ? [
            {
              device: str(v.device),
              id: num(v.id),
              freqHz: num(v.freq_hz),
              mode: str(v.mode),
              status: str(v.status),
            },
          ]
        : [],
    ),
  };
}

export function parseBan(raw: unknown): Ban {
  if (!isJson(raw)) return bad("ban");
  const state = raw.state;
  return {
    id: num(raw.id),
    network: str(raw.network),
    reason: strOrNull(raw.reason),
    state: state === "expired" || state === "lifted" ? state : "active",
    createdAtMs: num(raw.created_at_ms),
    createdBy: str(raw.created_by),
    expiresAtMs: numOrNull(raw.expires_at_ms),
    liftedAtMs: numOrNull(raw.lifted_at_ms),
    liftedBy: strOrNull(raw.lifted_by),
  };
}

export function parseMySession(raw: unknown): MySession {
  if (!isJson(raw) || typeof raw.sid !== "string") return bad("session");
  return {
    sid: raw.sid,
    ip: strOrNull(raw.ip),
    userAgent: strOrNull(raw.user_agent),
    client: parseClientInfo(raw),
    createdAtMs: num(raw.created_at_ms),
    lastSeenMs: num(raw.last_seen_ms),
    current: raw.current === true,
  };
}

const OUTCOMES: readonly Outcome[] = ["ok", "denied", "error"];
const ACTOR_KINDS: readonly ActorKind[] = ["user", "guest", "system"];

export function parseAuditEvent(raw: unknown): AuditEvent {
  if (!isJson(raw) || typeof raw.id !== "number") return bad("audit event");
  return {
    id: raw.id,
    tsMs: num(raw.ts_ms),
    actorKind: ACTOR_KINDS.find((k) => k === raw.actor_kind) ?? "system",
    actorId: strOrNull(raw.actor_id),
    actorName: strOrNull(raw.actor_name),
    role: isRole(raw.role) ? raw.role : null,
    ip: strOrNull(raw.ip),
    userAgent: strOrNull(raw.user_agent),
    client: parseClientInfo(raw),
    reported: isJson(raw.details) ? parseReportedClient(raw.details.client) : null,
    action: str(raw.action),
    target: strOrNull(raw.target),
    outcome: OUTCOMES.find((o) => o === raw.outcome) ?? "ok",
    details: isJson(raw.details) ? raw.details : {},
  };
}

export function parseAuditPage(raw: unknown): AuditPage {
  if (!isJson(raw)) return bad("audit page");
  return {
    events: list(raw.events).map(parseAuditEvent),
    nextBeforeId: numOrNull(raw.next_before_id),
  };
}

export const ALL_MODES = ["wfm", "nfm", "am", "usb", "lsb", "cw"] as const;

export const DEFAULT_GUEST_POLICY: GuestPolicy = {
  enabled: false,
  maxGuests: 5,
  maxGuestsPerIp: 2,
  maxVfosPerGuest: 1,
  allowedModes: ["wfm", "nfm", "am"],
  freqMinHz: null,
  freqMaxHz: null,
  idleTimeoutSecs: 600,
  onlyWithoutLock: false,
  showDecoderEvents: false,
  privacyText: "",
};

export function parseGuestPolicy(raw: unknown): GuestPolicy {
  const p = isJson(raw) && isJson(raw.guest_policy) ? raw.guest_policy : null;
  if (!p) return bad("guest policy");
  const d = DEFAULT_GUEST_POLICY;
  return {
    enabled: p.enabled === true,
    maxGuests: num(p.max_guests, d.maxGuests),
    maxGuestsPerIp: num(p.max_guests_per_ip, d.maxGuestsPerIp),
    maxVfosPerGuest: num(p.max_vfos_per_guest, d.maxVfosPerGuest),
    allowedModes: list(p.allowed_modes).filter((m): m is string => typeof m === "string"),
    freqMinHz: numOrNull(p.freq_min_hz),
    freqMaxHz: numOrNull(p.freq_max_hz),
    idleTimeoutSecs: num(p.idle_timeout_secs, d.idleTimeoutSecs),
    onlyWithoutLock: p.only_without_lock === true,
    showDecoderEvents: p.show_decoder_events === true,
    privacyText: str(p.privacy_text),
  };
}

export function guestPolicyBody(p: GuestPolicy): Json {
  return {
    enabled: p.enabled,
    max_guests: p.maxGuests,
    max_guests_per_ip: p.maxGuestsPerIp,
    max_vfos_per_guest: p.maxVfosPerGuest,
    allowed_modes: p.allowedModes,
    freq_min_hz: p.freqMinHz,
    freq_max_hz: p.freqMaxHz,
    idle_timeout_secs: p.idleTimeoutSecs,
    only_without_lock: p.onlyWithoutLock,
    show_decoder_events: p.showDecoderEvents,
    privacy_text: p.privacyText,
  };
}

export type CollectScope = "off" | "users" | "all";
export const COLLECT_SCOPES: readonly CollectScope[] = ["off", "users", "all"];

/** Whose browsers report device details, and whether they also look up their public address. */
export interface ClientReportPolicy {
  collect: CollectScope;
  askPublicIp: boolean;
  publicIpService: string;
}

export const DEFAULT_PUBLIC_IP_SERVICE = "https://api.ipify.org?format=json";

export const DEFAULT_CLIENT_REPORT_POLICY: ClientReportPolicy = {
  collect: "off",
  askPublicIp: false,
  publicIpService: DEFAULT_PUBLIC_IP_SERVICE,
};

export function parseClientReportPolicy(raw: unknown): ClientReportPolicy {
  const p = isJson(raw) && isJson(raw.client_report) ? raw.client_report : null;
  if (!p) return bad("client report policy");
  return {
    collect: COLLECT_SCOPES.find((c) => c === p.collect) ?? "off",
    askPublicIp: p.ask_public_ip === true,
    publicIpService: str(p.public_ip_service, DEFAULT_PUBLIC_IP_SERVICE),
  };
}

export function clientReportPolicyBody(p: ClientReportPolicy): Json {
  return {
    collect: p.collect,
    ask_public_ip: p.askPublicIp,
    public_ip_service: p.publicIpService,
  };
}

const IP_MODES: readonly IpMode[] = ["full", "truncated", "none"];

export function parseAuditSettings(raw: unknown): {
  settings: AuditSettings;
  stats: AuditStats | null;
} {
  const s = isJson(raw) && isJson(raw.audit_settings) ? raw.audit_settings : null;
  if (!s) return bad("audit settings");
  const st = isJson(raw) && isJson(raw.stats) ? raw.stats : null;
  return {
    settings: {
      retentionUserDays: num(s.retention_user_days, 90),
      retentionGuestDays: num(s.retention_guest_days, 7),
      ipModeUser: IP_MODES.find((m) => m === s.ip_mode_user) ?? "full",
      ipModeGuest: IP_MODES.find((m) => m === s.ip_mode_guest) ?? "truncated",
    },
    stats: st
      ? { count: num(st.count), oldestMs: numOrNull(st.oldest_ms), bytes: num(st.bytes) }
      : null,
  };
}

export function auditSettingsBody(s: AuditSettings): Json {
  return {
    retention_user_days: s.retentionUserDays,
    retention_guest_days: s.retentionGuestDays,
    ip_mode_user: s.ipModeUser,
    ip_mode_guest: s.ipModeGuest,
  };
}

export function parseLimits(raw: unknown): Limits {
  const l = isJson(raw) && isJson(raw.limits) ? raw.limits : null;
  if (!l) return bad("limits");
  return {
    listenerMaxVfos: num(l.listener_max_vfos, 4),
    operatorMaxVfos: num(l.operator_max_vfos),
  };
}

// ---------- queries ----------

/** Query string of the audit filters (also used by the export link). */
export function auditQuery(
  f: AuditFilter,
  extra: Record<string, string | number | undefined> = {},
): string {
  const q = new URLSearchParams();
  const set = (k: string, v: string | number | undefined) => {
    if (v !== undefined && v !== "") q.set(k, String(v));
  };
  set("from_ms", f.fromMs);
  set("to_ms", f.toMs);
  set("actor_kind", f.actorKind);
  set("actor", f.actor?.trim());
  set("role", f.role);
  set("action", f.action?.trim());
  set("outcome", f.outcome);
  set("ip", f.ip?.trim());
  set("target", f.target?.trim());
  for (const [k, v] of Object.entries(extra)) set(k, v);
  const s = q.toString();
  return s === "" ? "" : `?${s}`;
}

// ---------- client ----------

export interface AdminApi {
  listUsers(signal?: AbortSignal): Promise<User[]>;
  createUser(req: {
    username: string;
    role: Role;
    password?: string;
  }): Promise<{ user: User; password: string | null }>;
  updateUser(id: number, patch: { role?: Role; disabled?: boolean }): Promise<User>;
  deleteUser(id: number): Promise<void>;
  /** Without `password` the server generates one and returns it once. */
  resetPassword(id: number, password?: string): Promise<string | null>;
  listInvites(signal?: AbortSignal): Promise<Invite[]>;
  createInvite(req: { role: Role; expiresInHours?: number; note?: string }): Promise<CreatedInvite>;
  revokeInvite(id: number): Promise<void>;
  listSessions(signal?: AbortSignal): Promise<LiveSession[]>;
  kickSession(sid: string): Promise<void>;
  listBans(all?: boolean, signal?: AbortSignal): Promise<Ban[]>;
  /** Resolves with the new ban and the number of sessions that were ended. */
  createBan(req: BanRequest): Promise<{ ban: Ban; kicked: number }>;
  liftBan(id: number): Promise<void>;
  audit(
    filter: AuditFilter,
    page: { limit?: number; beforeId?: number },
    signal?: AbortSignal,
  ): Promise<AuditPage>;
  /** URL of the export download (the browser handles the attachment). */
  exportUrl(filter: AuditFilter, format: ExportFormat): string;
  getGuestPolicy(signal?: AbortSignal): Promise<GuestPolicy>;
  putGuestPolicy(p: GuestPolicy): Promise<GuestPolicy>;
  getAuditSettings(
    signal?: AbortSignal,
  ): Promise<{ settings: AuditSettings; stats: AuditStats | null }>;
  putAuditSettings(s: AuditSettings): Promise<AuditSettings>;
  getLimits(signal?: AbortSignal): Promise<Limits>;
  putLimits(l: Limits): Promise<Limits>;
  getClientReportPolicy(signal?: AbortSignal): Promise<ClientReportPolicy>;
  putClientReportPolicy(p: ClientReportPolicy): Promise<ClientReportPolicy>;
}

export interface SelfApi {
  list(signal?: AbortSignal): Promise<MySession[]>;
  revoke(sid: string): Promise<void>;
}

const ADMIN = "/api/v1/admin";

export function createAdminApi(fetchImpl?: FetchLike): AdminApi {
  const call = (method: string, url: string, body?: unknown, signal?: AbortSignal) =>
    request(method, url, { body, signal, fetchImpl });
  const id = encodeURIComponent;

  return {
    async listUsers(signal) {
      const r = await call("GET", `${ADMIN}/users`, undefined, signal);
      return isJson(r.body) ? list(r.body.users).map(parseUser) : bad("users");
    },
    async createUser({ username, role, password }) {
      const r = await call("POST", `${ADMIN}/users`, {
        username,
        role,
        ...(password ? { password } : {}),
      });
      if (!isJson(r.body)) return bad("create user");
      return { user: parseUser(r.body.user), password: strOrNull(r.body.password) };
    },
    async updateUser(userId, patch) {
      const r = await call("PATCH", `${ADMIN}/users/${userId}`, patch);
      return isJson(r.body) ? parseUser(r.body.user) : bad("user");
    },
    async deleteUser(userId) {
      await call("DELETE", `${ADMIN}/users/${userId}`);
    },
    async resetPassword(userId, password) {
      const r = await call(
        "POST",
        `${ADMIN}/users/${userId}/password`,
        password ? { password } : {},
      );
      return isJson(r.body) ? strOrNull(r.body.password) : null;
    },
    async listInvites(signal) {
      const r = await call("GET", `${ADMIN}/invites`, undefined, signal);
      return isJson(r.body) ? list(r.body.invites).map(parseInvite) : bad("invites");
    },
    async createInvite({ role, expiresInHours, note }) {
      const r = await call("POST", `${ADMIN}/invites`, {
        role,
        ...(expiresInHours !== undefined ? { expires_in_hours: expiresInHours } : {}),
        ...(note ? { note } : {}),
      });
      if (!isJson(r.body) || typeof r.body.token !== "string") return bad("create invite");
      return {
        invite: parseInvite(r.body.invite),
        token: r.body.token,
        path: str(r.body.path, `/invite/${r.body.token}`),
      };
    },
    async revokeInvite(inviteId) {
      await call("DELETE", `${ADMIN}/invites/${inviteId}`);
    },
    async listSessions(signal) {
      const r = await call("GET", `${ADMIN}/sessions`, undefined, signal);
      return isJson(r.body) ? list(r.body.sessions).map(parseLiveSession) : bad("sessions");
    },
    async kickSession(sid) {
      await call("DELETE", `${ADMIN}/sessions/${id(sid)}`);
    },
    async listBans(all, signal) {
      const r = await call("GET", `${ADMIN}/bans${all ? "?state=all" : ""}`, undefined, signal);
      return isJson(r.body) ? list(r.body.bans).map(parseBan) : bad("bans");
    },
    async createBan({ target, reason, expiresInMinutes, kick }) {
      const r = await call("POST", `${ADMIN}/bans`, {
        ...target,
        ...(reason ? { reason } : {}),
        ...(expiresInMinutes !== undefined ? { expires_in_minutes: expiresInMinutes } : {}),
        ...(kick !== undefined ? { kick } : {}),
      });
      if (!isJson(r.body)) return bad("create ban");
      return { ban: parseBan(r.body.ban), kicked: num(r.body.kicked) };
    },
    async liftBan(banId) {
      await call("DELETE", `${ADMIN}/bans/${banId}`);
    },
    async audit(filter, page, signal) {
      const r = await call(
        "GET",
        `${ADMIN}/audit${auditQuery(filter, { limit: page.limit, before_id: page.beforeId })}`,
        undefined,
        signal,
      );
      return parseAuditPage(r.body);
    },
    exportUrl(filter, format) {
      return `${ADMIN}/audit/export${auditQuery(filter, { format })}`;
    },
    async getGuestPolicy(signal) {
      return parseGuestPolicy((await call("GET", `${ADMIN}/guest-policy`, undefined, signal)).body);
    },
    async putGuestPolicy(p) {
      return parseGuestPolicy(
        (await call("PUT", `${ADMIN}/guest-policy`, guestPolicyBody(p))).body,
      );
    },
    async getAuditSettings(signal) {
      return parseAuditSettings(
        (await call("GET", `${ADMIN}/audit-settings`, undefined, signal)).body,
      );
    },
    async putAuditSettings(s) {
      return parseAuditSettings(
        (await call("PUT", `${ADMIN}/audit-settings`, auditSettingsBody(s))).body,
      ).settings;
    },
    async getClientReportPolicy(signal) {
      return parseClientReportPolicy(
        (await call("GET", `${ADMIN}/client-report-policy`, undefined, signal)).body,
      );
    },
    async putClientReportPolicy(p) {
      return parseClientReportPolicy(
        (await call("PUT", `${ADMIN}/client-report-policy`, clientReportPolicyBody(p))).body,
      );
    },
    async getLimits(signal) {
      return parseLimits((await call("GET", `${ADMIN}/limits`, undefined, signal)).body);
    },
    async putLimits(l) {
      return parseLimits(
        (
          await call("PUT", `${ADMIN}/limits`, {
            listener_max_vfos: l.listenerMaxVfos,
            operator_max_vfos: l.operatorMaxVfos,
          })
        ).body,
      );
    },
  };
}

export function createSelfApi(fetchImpl?: FetchLike): SelfApi {
  return {
    async list(signal) {
      const r = await request("GET", "/api/v1/me/sessions", { signal, fetchImpl });
      return isJson(r.body) ? list(r.body.sessions).map(parseMySession) : bad("sessions");
    },
    async revoke(sid) {
      await request("DELETE", `/api/v1/me/sessions/${encodeURIComponent(sid)}`, { fetchImpl });
    },
  };
}
