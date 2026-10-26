// Process-wide session state shared by the REST helpers and the WebSockets: once any request
// reports 401 the session is "lost" and the auth gate returns to the login page. Sockets consult
// the flag so they stop reconnecting while logged out.

const AUTH_PREFIX = "/api/v1/auth/";
const STATUS_URL = "/api/v1/auth/status";

let lost = false;
let verifying = false;
const listeners = new Set<() => void>();

export function isSessionLost(): boolean {
  return lost;
}

/** Mark the session as lost (idempotent) and tell the auth gate. */
export function notifyUnauthorized(): void {
  if (lost) return;
  lost = true;
  for (const l of [...listeners]) l();
}

/** Called after a successful login/setup so requests and sockets may run again. */
export function resetSession(): void {
  lost = false;
}

export function onSessionLost(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

// The auth gate re-reads /auth/status when the server says something about the session changed
// (a socket closed, or a data route answered `password_change_required`).
const statusListeners = new Set<(raw: unknown) => void>();
const recheckListeners = new Set<() => void>();

/** Receives the raw `/auth/status` document every time a session check saw a signed-in caller. */
export function onSessionStatus(listener: (raw: unknown) => void): () => void {
  statusListeners.add(listener);
  return () => statusListeners.delete(listener);
}

/** The gate should re-read the status (e.g. the account was forced to change its password). */
export function onRecheckRequested(listener: () => void): () => void {
  recheckListeners.add(listener);
  return () => recheckListeners.delete(listener);
}

export function requestRecheck(): void {
  for (const l of [...recheckListeners]) l();
}

function urlOf(input: RequestInfo | URL): string {
  if (typeof input === "string") return input;
  if (input instanceof URL) return input.pathname;
  return input.url;
}

/**
 * fetch for the data API: same-origin credentials, and a 401 anywhere outside /api/v1/auth/
 * (where 401 means "wrong password") ends the session.
 */
export async function apiFetch(input: RequestInfo | URL, init?: RequestInit): Promise<Response> {
  const res = await fetch(input, { credentials: "same-origin", ...init });
  if (res.status === 401 && !urlOf(input).startsWith(AUTH_PREFIX)) notifyUnauthorized();
  else if (res.status === 403 && !urlOf(input).startsWith(AUTH_PREFIX)) void checkForcedChange(res);
  return res;
}

/** A 403 `password_change_required` means the account must change its password first. */
async function checkForcedChange(res: Response): Promise<void> {
  try {
    const body: unknown = await res.clone().json();
    if (typeof body === "object" && body !== null && "error" in body) {
      if (body.error === "password_change_required") requestRecheck();
    }
  } catch {
    // Not JSON: an ordinary refusal.
  }
}

/**
 * A rejected WebSocket upgrade looks like any other failed connection to the browser. Sockets call
 * this when a connection closed without ever opening; if the server says we are not signed in the
 * session is marked lost (which stops the sockets). Single flight; network errors are ignored.
 */
export async function verifySession(
  fetchImpl: typeof fetch = (...a) => fetch(...a),
): Promise<void> {
  if (verifying || lost) return;
  verifying = true;
  try {
    const res = await fetchImpl(STATUS_URL, { credentials: "same-origin" });
    if (res.status === 401) {
      notifyUnauthorized();
      return;
    }
    if (!res.ok) return;
    const body: unknown = await res.json();
    if (typeof body === "object" && body !== null && "authenticated" in body) {
      if (body.authenticated === false) notifyUnauthorized();
      else for (const l of [...statusListeners]) l(body);
    }
  } catch {
    // Backend unreachable: keep the normal reconnect backoff.
  } finally {
    verifying = false;
  }
}

/**
 * A WebSocket closed. That can be a network blip, but also a kick, a disabled account or a reset
 * password (the server closes the sockets at once): ask the server who we are. The sockets call
 * this on every close; the check is single flight, the reconnect backoff spaces the closes, and
 * an ended session stops the reconnects (`isSessionLost`).
 */
export function checkSessionAfterClose(): void {
  void verifySession();
}
