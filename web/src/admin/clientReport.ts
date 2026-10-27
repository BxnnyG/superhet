// What a browser reported about itself at session start (docs/api-users.md, "Client report").
// Everything here is claimed by the client and unverified; the UI says so wherever it shows it.

import { isJson } from "../http";
import { parseAddrInfo, type AddrInfo } from "./clientInfo";

export interface ReportedBrand {
  brand: string;
  version: string;
}

export interface ReportedConnection {
  kind: string | null;
  effectiveType: string | null;
  downlinkMbps: number | null;
  saveData: boolean | null;
}

export type PublicIpStatus = "ok" | "blocked" | "failed";

export interface ReportedClient {
  platform: string | null;
  platformVersion: string | null;
  model: string | null;
  architecture: string | null;
  bitness: string | null;
  mobile: boolean | null;
  brands: ReportedBrand[];
  screen: { width: number; height: number; dpr: number } | null;
  languages: string[];
  timezone: string | null;
  touchPoints: number | null;
  cpuCores: number | null;
  memoryGb: number | null;
  connection: ReportedConnection | null;
  /** The public address the browser got from the lookup service (shown in the stored mode). */
  publicIp: string | null;
  publicIpInfo: AddrInfo | null;
  publicIpStatus: PublicIpStatus | null;
  reportedAtMs: number | null;
}

const s = (v: unknown): string | null => (typeof v === "string" && v !== "" ? v : null);
const n = (v: unknown): number | null => (typeof v === "number" && Number.isFinite(v) ? v : null);
const b = (v: unknown): boolean | null => (typeof v === "boolean" ? v : null);

/** Read the `client_report` object of a session or the `details.client` of an audit event. */
export function parseReportedClient(raw: unknown): ReportedClient | null {
  if (!isJson(raw)) return null;
  const screen =
    isJson(raw.screen) && n(raw.screen.width) !== null && n(raw.screen.height) !== null
      ? {
          width: n(raw.screen.width) ?? 0,
          height: n(raw.screen.height) ?? 0,
          dpr: n(raw.screen.dpr) ?? 1,
        }
      : null;
  const connection = isJson(raw.connection)
    ? {
        kind: s(raw.connection.kind),
        effectiveType: s(raw.connection.effective_type),
        downlinkMbps: n(raw.connection.downlink_mbps),
        saveData: b(raw.connection.save_data),
      }
    : null;
  const status = raw.public_ip_status;
  return {
    platform: s(raw.platform),
    platformVersion: s(raw.platform_version),
    model: s(raw.model),
    architecture: s(raw.architecture),
    bitness: s(raw.bitness),
    mobile: b(raw.mobile),
    brands: (Array.isArray(raw.brands) ? raw.brands : []).flatMap((x: unknown) =>
      isJson(x) && s(x.brand) ? [{ brand: s(x.brand) ?? "", version: s(x.version) ?? "" }] : [],
    ),
    screen,
    languages: (Array.isArray(raw.languages) ? raw.languages : []).filter(
      (l): l is string => typeof l === "string" && l !== "",
    ),
    timezone: s(raw.timezone),
    touchPoints: n(raw.touch_points),
    cpuCores: n(raw.cpu_cores),
    memoryGb: n(raw.memory_gb),
    connection,
    publicIp: s(raw.public_ip),
    publicIpInfo: parseAddrInfo(raw.public_ip_info),
    publicIpStatus: status === "ok" || status === "blocked" || status === "failed" ? status : null,
    reportedAtMs: n(raw.reported_at_ms),
  };
}

/** "Linux 6.12 · x86 64-bit · 2560×1440 @1.25×": one line for the session list. */
export function describeReportedDevice(r: ReportedClient): string {
  const os = [r.platform, r.platformVersion].filter(Boolean).join(" ");
  const cpu = [r.architecture, r.bitness ? `${r.bitness}-bit` : null].filter(Boolean).join(" ");
  const screen = r.screen
    ? `${r.screen.width}×${r.screen.height}${r.screen.dpr !== 1 ? ` @${r.screen.dpr}×` : ""}`
    : null;
  return [r.model, os, cpu, screen].filter((x) => x && x !== "").join(" · ");
}

export interface ReportRow {
  label: string;
  value: string;
}

function connectionText(c: ReportedConnection): string {
  const parts = [
    c.kind,
    c.effectiveType,
    c.downlinkMbps !== null ? `${c.downlinkMbps} Mbit/s` : null,
    c.saveData ? "data saver" : null,
  ];
  return parts.filter(Boolean).join(" · ");
}

/** The labelled rows of the details view, without the public address (shown separately). */
export function reportRows(r: ReportedClient): ReportRow[] {
  const rows: ReportRow[] = [];
  const add = (label: string, value: string | null) => {
    if (value && value !== "") rows.push({ label, value });
  };
  add("System", [r.platform, r.platformVersion].filter(Boolean).join(" "));
  add("Device model", r.model);
  add(
    "Device type",
    r.mobile === null ? null : r.mobile ? "Mobile" : r.touchPoints ? "Touch screen" : "Desktop",
  );
  add(
    "Architecture",
    [r.architecture, r.bitness ? `${r.bitness}-bit` : null].filter(Boolean).join(" "),
  );
  add("Browser engine", r.brands.map((x) => `${x.brand} ${x.version}`.trim()).join(", "));
  add(
    "Screen",
    r.screen
      ? `${r.screen.width}×${r.screen.height}, pixel ratio ${r.screen.dpr}${
          r.touchPoints ? `, ${r.touchPoints} touch points` : ""
        }`
      : null,
  );
  add("Languages", r.languages.join(", "));
  add("Time zone", r.timezone);
  add(
    "Hardware",
    [
      r.cpuCores !== null ? `${r.cpuCores} threads` : null,
      r.memoryGb !== null ? `≥ ${r.memoryGb} GB memory` : null,
    ]
      .filter(Boolean)
      .join(" · "),
  );
  add("Connection", r.connection ? connectionText(r.connection) : null);
  return rows;
}

/**
 * How the claimed public address relates to the address the server sees: `null` when either is
 * missing, "same" when they match, "differs" when they do not (a LAN or Docker address on the
 * server side is the usual reason, and the point of asking).
 */
export function compareAddresses(
  reported: string | null,
  seen: string | null,
): "same" | "differs" | null {
  if (!reported || !seen) return null;
  return reported === seen ? "same" : "differs";
}
