// JSON requests to the authenticated API (users, sessions, admin routes). Errors keep the HTTP
// status and the server's `error` code (docs/api-users.md: `{"error":"<code>","message":"..."}`).

import { apiFetch } from "./auth/session";

export class HttpError extends Error {
  constructor(
    readonly status: number,
    /** The server's error code (`username_taken`, `last_admin`, `forbidden`, ...), or "". */
    readonly code: string,
    message: string,
  ) {
    super(message);
    this.name = "HttpError";
  }
}

export type Json = Record<string, unknown>;

export function isJson(v: unknown): v is Json {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}

export interface Reply {
  status: number;
  /** Parsed JSON body; undefined for 204 or a non-JSON body. */
  body: unknown;
  headers: Headers;
}

/** Plain-language text for the codes admins meet; unknown codes show the server's message. */
const CODE_TEXT: Record<string, string> = {
  username_taken: "This username is already taken.",
  last_admin: "That would leave no active admin. Make another account an admin first.",
  not_found: "It no longer exists. Refresh the list.",
  invalid: "The server rejected the values.",
  forbidden: "Your role is not allowed to do that.",
  unauthenticated: "You are signed out.",
  invalid_invite: "The invitation is no longer open.",
  rate_limited: "Too many requests. Try again in a moment.",
  unavailable: "The server is not available.",
};

export function describeHttpError(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

async function readBody(res: Response): Promise<unknown> {
  if (res.status === 204) return undefined;
  try {
    return await res.json();
  } catch {
    return undefined;
  }
}

export type FetchLike = (input: RequestInfo | URL, init?: RequestInit) => Promise<Response>;

/** Send a request; resolves with the reply, rejects with HttpError for any non-2xx status. */
export async function request(
  method: string,
  url: string,
  opts: { body?: unknown; signal?: AbortSignal; fetchImpl?: FetchLike } = {},
): Promise<Reply> {
  const fetchImpl = opts.fetchImpl ?? apiFetch;
  let res: Response;
  try {
    res = await fetchImpl(url, {
      method,
      headers: opts.body === undefined ? undefined : { "Content-Type": "application/json" },
      body: opts.body === undefined ? undefined : JSON.stringify(opts.body),
      signal: opts.signal,
    });
  } catch (e) {
    if (e instanceof DOMException && e.name === "AbortError") throw e;
    throw new HttpError(0, "network", "Cannot reach the server.");
  }
  const body = await readBody(res);
  if (!res.ok) {
    const code = isJson(body) && typeof body.error === "string" ? body.error : "";
    const msg = isJson(body) && typeof body.message === "string" ? body.message : "";
    throw new HttpError(
      res.status,
      code,
      msg || CODE_TEXT[code] || `Request failed (HTTP ${res.status}).`,
    );
  }
  return { status: res.status, body, headers: res.headers };
}
