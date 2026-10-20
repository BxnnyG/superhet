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
  return res;
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
    }
  } catch {
    // Backend unreachable: keep the normal reconnect backoff.
  } finally {
    verifying = false;
  }
}
