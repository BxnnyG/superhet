// What this browser tells the server about itself at session start, when the admin policy asks
// for it (docs/api-users.md, "Client report"): User-Agent Client Hints, screen, languages, time
// zone, hardware hints, connection type and, if requested, the public address from a lookup
// service. Everything is best effort; a missing API just leaves the field out. The server marks
// all of it as client-reported and unverified.

import { request } from "../http";

/** What the server asks this browser for (`client_report` of `GET /api/v1/auth/status`). */
export interface ClientReportRequest {
  /** URL to look the public address up at, or null when only device details are wanted. */
  publicIpService: string | null;
}

export type PublicIpStatus = "ok" | "blocked" | "failed";

/** Wire format of `POST /api/v1/auth/client-report` (snake_case, every field optional). */
export interface ClientReportPayload {
  platform?: string;
  platform_version?: string;
  model?: string;
  architecture?: string;
  bitness?: string;
  mobile?: boolean;
  brands?: { brand: string; version: string }[];
  screen?: { width: number; height: number; dpr: number };
  languages?: string[];
  timezone?: string;
  touch_points?: number;
  cpu_cores?: number;
  memory_gb?: number;
  connection?: {
    kind?: string;
    effective_type?: string;
    downlink_mbps?: number;
    save_data?: boolean;
  };
  public_ip?: string;
  public_ip_status?: PublicIpStatus;
}

interface UaBrand {
  brand: string;
  version: string;
}

interface UaData {
  mobile?: boolean;
  platform?: string;
  getHighEntropyValues?: (hints: string[]) => Promise<Record<string, unknown>>;
}

interface NetworkInformation {
  type?: string;
  effectiveType?: string;
  downlink?: number;
  saveData?: boolean;
}

/** The parts of the browser this module reads; injectable for tests. */
export interface DeviceEnv {
  navigator: {
    userAgentData?: UaData;
    platform?: string;
    languages?: readonly string[];
    language?: string;
    maxTouchPoints?: number;
    hardwareConcurrency?: number;
    deviceMemory?: number;
    connection?: NetworkInformation;
  };
  screen: { width: number; height: number } | undefined;
  devicePixelRatio: number;
  timeZone: () => string | undefined;
}

export function browserEnv(): DeviceEnv {
  return {
    navigator: navigator as unknown as DeviceEnv["navigator"],
    screen: typeof screen === "undefined" ? undefined : screen,
    devicePixelRatio: window.devicePixelRatio || 1,
    timeZone: () => {
      try {
        return Intl.DateTimeFormat().resolvedOptions().timeZone;
      } catch {
        return undefined;
      }
    },
  };
}

const HIGH_ENTROPY = [
  "platformVersion",
  "model",
  "architecture",
  "bitness",
  "fullVersionList",
] as const;
const HINTS_TIMEOUT_MS = 1500;

const str = (v: unknown): string | undefined =>
  typeof v === "string" && v.trim() !== "" ? v.trim() : undefined;
const num = (v: unknown): number | undefined =>
  typeof v === "number" && Number.isFinite(v) ? v : undefined;

function withTimeout<T>(p: Promise<T>, ms: number): Promise<T | undefined> {
  return new Promise((resolve) => {
    const timer = setTimeout(() => resolve(undefined), ms);
    p.then(
      (v) => {
        clearTimeout(timer);
        resolve(v);
      },
      () => {
        clearTimeout(timer);
        resolve(undefined);
      },
    );
  });
}

function brandsOf(v: unknown): UaBrand[] {
  if (!Array.isArray(v)) return [];
  return v
    .flatMap((b: unknown) => {
      const o = b as { brand?: unknown; version?: unknown } | null;
      const brand = str(o?.brand);
      // "Not A;Brand" style GREASE entries carry no information.
      return brand && !/not.?a.?brand/i.test(brand)
        ? [{ brand, version: str(o?.version) ?? "" }]
        : [];
    })
    .slice(0, 12);
}

/** Gather everything the browser is willing to say; never throws. */
export async function collectDeviceInfo(
  env: DeviceEnv = browserEnv(),
): Promise<ClientReportPayload> {
  const out: ClientReportPayload = {};
  const nav = env.navigator;
  const ua = nav.userAgentData;
  const platform = str(ua?.platform) ?? str(nav.platform);
  if (platform) out.platform = platform;
  if (typeof ua?.mobile === "boolean") out.mobile = ua.mobile;

  if (ua?.getHighEntropyValues) {
    const high = await withTimeout(ua.getHighEntropyValues([...HIGH_ENTROPY]), HINTS_TIMEOUT_MS);
    if (high) {
      const v = str(high.platformVersion);
      if (v) out.platform_version = v;
      const model = str(high.model);
      if (model) out.model = model;
      const arch = str(high.architecture);
      if (arch) out.architecture = arch;
      const bits = str(high.bitness);
      if (bits) out.bitness = bits;
      const brands = brandsOf(high.fullVersionList);
      if (brands.length > 0) out.brands = brands;
    }
  }

  if (env.screen && env.screen.width > 0 && env.screen.height > 0) {
    out.screen = {
      width: Math.round(env.screen.width),
      height: Math.round(env.screen.height),
      dpr: env.devicePixelRatio > 0 ? env.devicePixelRatio : 1,
    };
  }
  const langs = (nav.languages && nav.languages.length > 0 ? [...nav.languages] : [nav.language])
    .filter((l): l is string => typeof l === "string" && l !== "")
    .slice(0, 12);
  if (langs.length > 0) out.languages = langs;
  const tz = str(env.timeZone());
  if (tz) out.timezone = tz;
  const touch = num(nav.maxTouchPoints);
  if (touch !== undefined) out.touch_points = Math.max(0, Math.round(touch));
  const cores = num(nav.hardwareConcurrency);
  if (cores !== undefined && cores >= 1) out.cpu_cores = Math.round(cores);
  const mem = num(nav.deviceMemory);
  if (mem !== undefined && mem > 0) out.memory_gb = mem;

  const c = nav.connection;
  if (c) {
    const conn: NonNullable<ClientReportPayload["connection"]> = {};
    const kind = str(c.type);
    if (kind) conn.kind = kind;
    const eff = str(c.effectiveType);
    if (eff) conn.effective_type = eff;
    const down = num(c.downlink);
    if (down !== undefined && down >= 0) conn.downlink_mbps = down;
    if (typeof c.saveData === "boolean") conn.save_data = c.saveData;
    if (Object.keys(conn).length > 0) out.connection = conn;
  }
  return out;
}

const IP_TEXT = /^[0-9a-fA-F:.]{2,45}$/;

/** The address in a lookup service's answer: `{"ip": "..."}` or a bare address. */
export function parseIpAnswer(body: string): string | null {
  const text = body.trim();
  if (text === "") return null;
  let candidate = text;
  if (text.startsWith("{")) {
    try {
      const parsed: unknown = JSON.parse(text);
      const ip = (parsed as { ip?: unknown } | null)?.ip;
      candidate = typeof ip === "string" ? ip.trim() : "";
    } catch {
      return null;
    }
  }
  return IP_TEXT.test(candidate) && (candidate.includes(".") || candidate.includes(":"))
    ? candidate
    : null;
}

export interface PublicIpResult {
  ip: string | null;
  status: PublicIpStatus;
}

/**
 * Ask `service` for this browser's public address. No cookies, no referrer, no cache. A blocked
 * request (content security policy, ad blocker, offline) is reported as `blocked`, anything the
 * service answers wrongly or too slowly as `failed`.
 */
export async function fetchPublicIp(
  service: string,
  fetchImpl: typeof fetch = (...a) => fetch(...a),
  timeoutMs = 4000,
): Promise<PublicIpResult> {
  const ctl = new AbortController();
  const timer = setTimeout(() => ctl.abort(), timeoutMs);
  try {
    const res = await fetchImpl(service, {
      method: "GET",
      credentials: "omit",
      referrerPolicy: "no-referrer",
      cache: "no-store",
      signal: ctl.signal,
    });
    if (!res.ok) return { ip: null, status: "failed" };
    const ip = parseIpAnswer((await res.text()).slice(0, 512));
    return ip ? { ip, status: "ok" } : { ip: null, status: "failed" };
  } catch (e) {
    const timedOut = e instanceof DOMException && e.name === "AbortError";
    return { ip: null, status: timedOut ? "failed" : "blocked" };
  } finally {
    clearTimeout(timer);
  }
}

export interface ReportDeps {
  env?: DeviceEnv;
  fetchImpl?: typeof fetch;
  send?: (payload: ClientReportPayload) => Promise<void>;
}

const sendToServer = async (payload: ClientReportPayload): Promise<void> => {
  await request("POST", "/api/v1/auth/client-report", { body: payload });
};

/** Collect, look up the public address when asked, and send. Best effort: errors are swallowed. */
export async function runClientReport(
  ask: ClientReportRequest,
  deps: ReportDeps = {},
): Promise<ClientReportPayload | null> {
  try {
    const payload = await collectDeviceInfo(deps.env);
    if (ask.publicIpService) {
      const found = await fetchPublicIp(ask.publicIpService, deps.fetchImpl);
      if (found.ip) payload.public_ip = found.ip;
      payload.public_ip_status = found.status;
    }
    await (deps.send ?? sendToServer)(payload);
    return payload;
  } catch {
    return null;
  }
}
