// Typed client for the auth routes (/api/v1/auth/*, docs/api-auth.md and docs/api-users.md).

import type { ClientReportRequest } from "../clientreport/collect";

export type Role = "admin" | "operator" | "listener";
export const ROLES: readonly Role[] = ["admin", "operator", "listener"];

export function isRole(v: unknown): v is Role {
  return ROLES.some((r) => r === v);
}

export interface GuestInfo {
  enabled: boolean;
  /** The notice shown before a guest session starts; only set while guest mode is on. */
  privacyText: string | null;
  /** What a guest's browser will be asked to report (device details, public address lookup). */
  clientReport?: ClientReportRequest | null;
}

export interface AuthStatus {
  setupRequired: boolean;
  authenticated: boolean;
  /** null for guests and signed-out callers. */
  username: string | null;
  userId: number | null;
  role: Role | null;
  kind: "user" | "guest" | null;
  /** The account still has a one-time password: only the password change works. */
  mustChangePassword: boolean;
  guest: GuestInfo;
  /** What this browser is asked to report about itself; null/absent: nothing. */
  clientReport?: ClientReportRequest | null;
}

/** What the guest policy allows (reply of `POST /api/v1/auth/guest`). */
export interface GuestLimits {
  idleTimeoutSecs: number;
  maxVfos: number;
  allowedModes: string[];
  freqMinHz: number | null;
  freqMaxHz: number | null;
  showDecoderEvents: boolean;
}

export interface InvitePreview {
  role: Role;
  expiresAtMs: number;
  invitedBy: string | null;
}

export type AuthErrorKind =
  | "invalid_credentials"
  | "rate_limited"
  | "already_setup"
  | "invalid_setup_token"
  | "invalid"
  | "unauthenticated"
  | "invalid_invite"
  | "username_taken"
  | "guests_full"
  | "too_many_guests"
  | "not_found"
  | "forbidden"
  | "password_change_required"
  | "unavailable"
  | "banned"
  | "network"
  | "unknown";

export class AuthError extends Error {
  constructor(
    readonly kind: AuthErrorKind,
    message: string,
    /** Seconds until a rate-limited request may be retried. */
    readonly retryAfter?: number,
  ) {
    super(message);
    this.name = "AuthError";
  }
}

export interface AuthClient {
  status(signal?: AbortSignal): Promise<AuthStatus>;
  setup(req: { username: string; password: string; setupToken: string }): Promise<void>;
  login(req: { username: string; password: string }): Promise<void>;
  logout(): Promise<void>;
  changePassword(req: { currentPassword: string; newPassword: string }): Promise<void>;
  /** Start (or resume) a guest session; the reply carries the policy limits. */
  guestStart(): Promise<GuestLimits>;
  previewInvite(token: string): Promise<InvitePreview>;
  acceptInvite(req: { token: string; username: string; password: string }): Promise<void>;
}

export const USERNAME_RE = /^[A-Za-z0-9._-]{1,64}$/;
export const PASSWORD_MIN = 12;
export const PASSWORD_MAX = 1024;
const DEFAULT_RETRY_S = 30;

type Rec = Record<string, unknown>;

function isRec(v: unknown): v is Rec {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}

const NO_GUEST: GuestInfo = { enabled: false, privacyText: null };

export function parseStatus(raw: unknown): AuthStatus {
  if (!isRec(raw)) throw new AuthError("unknown", "invalid status");
  if (typeof raw.setup_required !== "boolean" || typeof raw.authenticated !== "boolean") {
    throw new AuthError("unknown", "invalid status");
  }
  const kind = raw.kind === "user" || raw.kind === "guest" ? raw.kind : null;
  const guest = isRec(raw.guest)
    ? {
        enabled: raw.guest.enabled === true,
        privacyText: typeof raw.guest.privacy_text === "string" ? raw.guest.privacy_text : null,
        ...withReport(parseClientReportRequest(raw.guest.client_report)),
      }
    : NO_GUEST;
  let role: Role | null = isRole(raw.role) ? raw.role : null;
  // A signed-in caller without a role (old single-user API) is the admin.
  if (raw.authenticated && role === null) role = kind === "guest" ? "listener" : "admin";
  return {
    setupRequired: raw.setup_required,
    authenticated: raw.authenticated,
    username: typeof raw.username === "string" ? raw.username : null,
    userId: typeof raw.user_id === "number" ? raw.user_id : null,
    role: raw.authenticated ? role : null,
    kind: raw.authenticated ? (kind ?? "user") : null,
    mustChangePassword: raw.must_change_password === true,
    guest,
    ...withReport(raw.authenticated ? parseClientReportRequest(raw.client_report) : null),
  };
}

/** The `clientReport` property only when the server asks for something. */
function withReport(r: ClientReportRequest | null): { clientReport?: ClientReportRequest } {
  return r ? { clientReport: r } : {};
}

/** `client_report` of the status reply: `{public_ip_service: string | null}` or null. */
export function parseClientReportRequest(raw: unknown): ClientReportRequest | null {
  if (!isRec(raw)) return null;
  const service = typeof raw.public_ip_service === "string" ? raw.public_ip_service : null;
  return { publicIpService: service !== null && service.startsWith("https://") ? service : null };
}

function numOrNull(v: unknown): number | null {
  return typeof v === "number" && Number.isFinite(v) ? v : null;
}

export function parseGuestLimits(raw: unknown): GuestLimits {
  const l = isRec(raw) && isRec(raw.limits) ? raw.limits : null;
  if (!l) throw new AuthError("unknown", "invalid guest reply");
  return {
    idleTimeoutSecs: numOrNull(l.idle_timeout_secs) ?? 600,
    maxVfos: numOrNull(l.max_vfos) ?? 1,
    allowedModes: Array.isArray(l.allowed_modes)
      ? l.allowed_modes.filter((m): m is string => typeof m === "string")
      : [],
    freqMinHz: numOrNull(l.freq_min_hz),
    freqMaxHz: numOrNull(l.freq_max_hz),
    showDecoderEvents: l.show_decoder_events === true,
  };
}

export function parseInvitePreview(raw: unknown): InvitePreview {
  if (!isRec(raw) || !isRole(raw.role)) throw new AuthError("unknown", "invalid invite");
  return {
    role: raw.role,
    expiresAtMs: numOrNull(raw.expires_at_ms) ?? 0,
    invitedBy: typeof raw.invited_by === "string" ? raw.invited_by : null,
  };
}

async function readBody(res: Response): Promise<Rec> {
  try {
    const b: unknown = await res.json();
    return isRec(b) ? b : {};
  } catch {
    return {};
  }
}

function retryAfterSeconds(res: Response): number {
  const n = Number(res.headers.get("Retry-After"));
  return Number.isFinite(n) && n > 0 ? Math.ceil(n) : DEFAULT_RETRY_S;
}

async function toError(res: Response): Promise<AuthError> {
  const body = await readBody(res);
  const code = typeof body.error === "string" ? body.error : "";
  const msg = typeof body.message === "string" ? body.message : "";
  switch (res.status) {
    case 400:
      return new AuthError("invalid", msg || "The request was rejected as invalid.");
    case 401:
      return code === "unauthenticated"
        ? new AuthError("unauthenticated", "You are signed out.")
        : new AuthError("invalid_credentials", "Invalid username or password.");
    case 403:
      if (code === "password_change_required") {
        return new AuthError("password_change_required", "You must change your password first.");
      }
      if (code === "banned") {
        return new AuthError("banned", "This address is blocked by the administrator.");
      }
      return code === "forbidden"
        ? new AuthError("forbidden", "You are not allowed to do that.")
        : new AuthError("invalid_setup_token", "The setup token is not valid.");
    case 404:
      return code === "invalid_invite"
        ? new AuthError("invalid_invite", "This invitation link is not valid any more.")
        : new AuthError("not_found", "Not available.");
    case 409:
      return code === "username_taken"
        ? new AuthError("username_taken", "This username is already taken.")
        : new AuthError("already_setup", "Setup was already completed.");
    case 429:
      return code === "too_many_guests"
        ? new AuthError("too_many_guests", "Too many guest sessions from your address.")
        : new AuthError("rate_limited", "Too many attempts.", retryAfterSeconds(res));
    case 503:
      return code === "guests_full"
        ? new AuthError("guests_full", "All guest places are taken right now.")
        : new AuthError("unavailable", "The server is not available.");
    default:
      return new AuthError("unknown", `Request failed (HTTP ${res.status}).`);
  }
}

export function createAuthClient(fetchImpl: typeof fetch = (...a) => fetch(...a)): AuthClient {
  async function call(url: string, method: "GET" | "POST", body?: Rec, signal?: AbortSignal) {
    let res: Response;
    try {
      res = await fetchImpl(url, {
        method,
        credentials: "same-origin",
        headers: body ? { "Content-Type": "application/json" } : undefined,
        body: body ? JSON.stringify(body) : undefined,
        signal,
      });
    } catch (e) {
      if (e instanceof DOMException && e.name === "AbortError") throw e;
      throw new AuthError("network", "Cannot reach the server.");
    }
    return res;
  }
  async function expectOk(res: Response): Promise<void> {
    if (!res.ok) throw await toError(res);
  }

  return {
    async status(signal) {
      const res = await call("/api/v1/auth/status", "GET", undefined, signal);
      if (!res.ok) throw await toError(res);
      return parseStatus(await readBody(res));
    },
    async setup({ username, password, setupToken }) {
      await expectOk(
        await call("/api/v1/auth/setup", "POST", {
          username,
          password,
          setup_token: setupToken,
        }),
      );
    },
    async login({ username, password }) {
      await expectOk(await call("/api/v1/auth/login", "POST", { username, password }));
    },
    async logout() {
      await expectOk(await call("/api/v1/auth/logout", "POST", {}));
    },
    async changePassword({ currentPassword, newPassword }) {
      const res = await call("/api/v1/auth/password", "POST", {
        current_password: currentPassword,
        new_password: newPassword,
      });
      if (res.status === 401) {
        const body = await readBody(res);
        if (body.error === "unauthenticated")
          throw new AuthError("unauthenticated", "You are signed out.");
        throw new AuthError("invalid_credentials", "The current password is not correct.");
      }
      await expectOk(res);
    },
    async guestStart() {
      const res = await call("/api/v1/auth/guest", "POST", {});
      if (!res.ok) throw await toError(res);
      return parseGuestLimits(await readBody(res));
    },
    async previewInvite(token) {
      const res = await call(`/api/v1/auth/invites/${encodeURIComponent(token)}`, "GET");
      if (!res.ok) throw await toError(res);
      return parseInvitePreview(await readBody(res));
    },
    async acceptInvite({ token, username, password }) {
      await expectOk(
        await call("/api/v1/auth/invites/accept", "POST", { token, username, password }),
      );
    },
  };
}
