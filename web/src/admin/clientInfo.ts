// Client details the admin API attaches to sessions and audit events (docs/api-users.md,
// docs/client-addresses.md): address type, reverse DNS, GeoIP, parsed user agent.

import { isJson } from "../http";

export type AddrClass =
  | "loopback"
  | "private"
  | "docker_gateway"
  | "cgnat"
  | "link_local"
  | "multicast"
  | "reserved"
  | "public";

const CLASSES: readonly AddrClass[] = [
  "loopback",
  "private",
  "docker_gateway",
  "cgnat",
  "link_local",
  "multicast",
  "reserved",
  "public",
];

export interface AddrInfo {
  class: AddrClass;
  /** "Private LAN", "Docker gateway", "Public", ... */
  label: string;
  family: "ipv4" | "ipv6";
  rdns: string | null;
  /** ISO 3166-1 alpha-2. */
  country: string | null;
  countryName: string | null;
  city: string | null;
  asn: number | null;
  org: string | null;
}

export interface UaInfo {
  browser: string | null;
  browserVersion: string | null;
  os: string | null;
  osVersion: string | null;
  device: "desktop" | "mobile" | "tablet" | "bot" | "tool" | "unknown";
}

/** Address data of one session or event. */
export interface ClientInfo {
  /** TCP peer address. */
  peerAddr: string | null;
  ipInfo: AddrInfo | null;
  peerInfo: AddrInfo | null;
  uaInfo: UaInfo | null;
}

const s = (v: unknown): string | null => (typeof v === "string" && v !== "" ? v : null);

export function parseAddrInfo(raw: unknown): AddrInfo | null {
  if (!isJson(raw)) return null;
  const cls = CLASSES.find((c) => c === raw.class);
  if (!cls) return null;
  return {
    class: cls,
    label: s(raw.label) ?? cls,
    family: raw.family === "ipv6" ? "ipv6" : "ipv4",
    rdns: s(raw.rdns),
    country: s(raw.country),
    countryName: s(raw.country_name),
    city: s(raw.city),
    asn: typeof raw.asn === "number" ? raw.asn : null,
    org: s(raw.org),
  };
}

const DEVICES = ["desktop", "mobile", "tablet", "bot", "tool", "unknown"] as const;

export function parseUaInfo(raw: unknown): UaInfo | null {
  if (!isJson(raw)) return null;
  return {
    browser: s(raw.browser),
    browserVersion: s(raw.browser_version),
    os: s(raw.os),
    osVersion: s(raw.os_version),
    device: DEVICES.find((d) => d === raw.device) ?? "unknown",
  };
}

/** Read `peer_addr`, `ip_info`, `peer_info` and `ua_info` of a session or event object. */
export function parseClientInfo(raw: Record<string, unknown>): ClientInfo {
  return {
    peerAddr: s(raw.peer_addr),
    ipInfo: parseAddrInfo(raw.ip_info),
    peerInfo: parseAddrInfo(raw.peer_info),
    uaInfo: parseUaInfo(raw.ua_info),
  };
}

/** "Public", "IPv6 public", "Private LAN", "IPv6 private LAN"... */
export function addrLabel(info: AddrInfo): string {
  if (info.family === "ipv4") return info.label;
  return `IPv6 ${info.label.charAt(0).toLowerCase()}${info.label.slice(1)}`.replace(
    "IPv6 cGNAT",
    "IPv6 CGNAT",
  );
}

/** "Chrome 131 on Windows 10/11 (desktop)"; the raw string when nothing was parsed. */
export function describeUa(info: UaInfo | null, raw: string | null): string {
  if (!info || (!info.browser && !info.os)) return raw ?? "Unknown browser";
  const browser = [info.browser, info.browserVersion].filter(Boolean).join(" ");
  const os = [info.os, info.osVersion].filter(Boolean).join(" ");
  const what = [browser || null, os ? (browser ? `on ${os}` : os) : null].filter(Boolean).join(" ");
  return info.device === "unknown" || info.device === "desktop" ? what : `${what} (${info.device})`;
}

/** "Berlin, DE", "Germany", "AS3320 Example Telecom"; empty when there is no GeoIP data. */
export function describeGeo(info: AddrInfo | null): string[] {
  if (!info) return [];
  const place = [info.city, info.country ?? info.countryName].filter(Boolean).join(", ");
  const network = info.asn !== null ? `AS${info.asn}${info.org ? ` ${info.org}` : ""}` : info.org;
  return [place, network ?? ""].filter((x) => x !== "");
}

/** The peer differs from the client address, i.e. a proxy forwarded the real one. */
export function viaProxy(ip: string | null, peer: string | null): boolean {
  return ip !== null && peer !== null && ip !== peer;
}

/**
 * Whether the recent addresses show that every visitor arrives through the Docker bridge
 * gateway: at least one known address and all of them are gateway addresses. Visitors without a
 * stored address (privacy mode "none") are ignored; a single real address clears the warning.
 */
export function dockerGatewayOnly(infos: readonly (AddrInfo | null)[]): boolean {
  const known = infos.filter((i): i is AddrInfo => i !== null);
  return known.length > 0 && known.every((i) => i.class === "docker_gateway");
}
