// Scanner control routes (docs/scanner.md): POST /api/v1/devices/{dev}/scanner/{action}. The new
// state arrives through the state socket; the reply only says whether the daemon accepted it.

import { apiFetch } from "../auth/session";
import { deviceUrl } from "../spectrum/api";

export type ScanAction = "start" | "stop" | "pause" | "resume" | "skip" | "lockout" | "unlock";

export interface ScannerApi {
  send(device: string, action: ScanAction, body?: Record<string, unknown>): Promise<void>;
}

export class ScanError extends Error {
  constructor(
    message: string,
    readonly status: number,
    readonly code: string,
  ) {
    super(message);
    this.name = "ScanError";
  }
}

/** The daemon's own words for a refusal, else a plain-language text for the HTTP status. */
export function describeScanError(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

export function createScannerApi(fetchImpl: typeof fetch = apiFetch): ScannerApi {
  return {
    async send(device, action, body = { v: 1 }) {
      let res: Response;
      try {
        res = await fetchImpl(`${deviceUrl(device)}/scanner/${action}`, {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify(body),
        });
      } catch {
        throw new ScanError("Cannot reach the server.", 0, "network");
      }
      if (res.ok) return;
      let parsed: unknown;
      try {
        parsed = await res.json();
      } catch {
        parsed = undefined;
      }
      const rec =
        typeof parsed === "object" && parsed !== null ? (parsed as Record<string, unknown>) : {};
      // The daemon answers {"ok":false,"code","error"}; the api's own refusals {"error":"<code>"}.
      const code =
        typeof rec.code === "string" ? rec.code : typeof rec.error === "string" ? rec.error : "";
      const text =
        typeof rec.code === "string" && typeof rec.error === "string"
          ? rec.error
          : res.status === 403
            ? "Only operators and admins can control the scanner."
            : res.status === 503 || res.status === 504
              ? "The receiver does not answer."
              : `Request failed (HTTP ${res.status}).`;
      throw new ScanError(text, res.status, code);
    },
  };
}
