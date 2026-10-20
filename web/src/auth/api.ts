// Typed client for the auth routes (/api/v1/auth/*).

export interface AuthStatus {
  setupRequired: boolean;
  authenticated: boolean;
  username: string | null;
}

export type AuthErrorKind =
  | "invalid_credentials"
  | "rate_limited"
  | "already_setup"
  | "invalid_setup_token"
  | "invalid"
  | "unauthenticated"
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
}

export const USERNAME_RE = /^[A-Za-z0-9._-]{1,64}$/;
export const PASSWORD_MIN = 12;
export const PASSWORD_MAX = 1024;
const DEFAULT_RETRY_S = 30;

export function parseStatus(raw: unknown): AuthStatus {
  if (typeof raw !== "object" || raw === null) throw new AuthError("unknown", "invalid status");
  const o = raw as Record<string, unknown>;
  if (typeof o.setup_required !== "boolean" || typeof o.authenticated !== "boolean") {
    throw new AuthError("unknown", "invalid status");
  }
  return {
    setupRequired: o.setup_required,
    authenticated: o.authenticated,
    username: typeof o.username === "string" ? o.username : null,
  };
}

async function readBody(res: Response): Promise<Record<string, unknown>> {
  try {
    const b: unknown = await res.json();
    return typeof b === "object" && b !== null ? (b as Record<string, unknown>) : {};
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
      return new AuthError("invalid_setup_token", "The setup token is not valid.");
    case 409:
      return new AuthError("already_setup", "Setup was already completed.");
    case 429:
      return new AuthError("rate_limited", "Too many attempts.", retryAfterSeconds(res));
    default:
      return new AuthError("unknown", `Request failed (HTTP ${res.status}).`);
  }
}

export function createAuthClient(fetchImpl: typeof fetch = (...a) => fetch(...a)): AuthClient {
  async function call(
    url: string,
    method: "GET" | "POST",
    body?: Record<string, unknown>,
    signal?: AbortSignal,
  ) {
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
  };
}
