// Where the maps (ADS-B aircraft, APRS stations) get its tiles from: `GET /api/v1/map-config` (docs/plugins-adsb.md). The
// server default is OpenStreetMap; `SUPERHET_MAP_TILE_URL` points it elsewhere and the CSP
// follows. By default the tiles come through the server's caching proxy (a same-origin path) so
// the browser never contacts the provider. An older server without the route simply gets the
// OpenStreetMap default, fetched directly.

import { useEffect, useState } from "react";
import { apiFetch } from "../auth/session";

export interface MapConfig {
  /** Tile URL template with {z}, {x} and {y}. */
  tileUrl: string;
  /** Tiles come through this server's proxy (the browser never contacts the tile provider). */
  proxied: boolean;
  /** Plain-text attribution the provider asks for (may be empty). */
  attribution: string;
  /** Link of the attribution; null for none. */
  attributionUrl: string | null;
}

/** OSM's standard tile server; its tile usage policy requires this attribution. */
export const DEFAULT_MAP: MapConfig = {
  tileUrl: "https://tile.openstreetmap.org/{z}/{x}/{y}.png",
  proxied: false,
  attribution: "© OpenStreetMap contributors",
  attributionUrl: "https://www.openstreetmap.org/copyright",
};

function text(v: unknown): string | null {
  return typeof v === "string" && v.trim() !== "" ? v.trim() : null;
}

function webLink(v: unknown): string | null {
  const s = text(v);
  return s !== null && /^https?:\/\//i.test(s) ? s : null;
}

/** A same-origin path such as the tile proxy's (not protocol-relative). */
function sameOriginPath(v: unknown): string | null {
  const s = text(v);
  return s !== null && /^\/(?!\/)/.test(s) ? s : null;
}

/** Validate a server reply; anything unusable falls back to the default source. */
export function parseMapConfig(raw: unknown): MapConfig {
  if (typeof raw !== "object" || raw === null) return DEFAULT_MAP;
  const o = raw as Record<string, unknown>;
  const tileUrl = webLink(o.tile_url) ?? sameOriginPath(o.tile_url);
  if (tileUrl === null) return DEFAULT_MAP;
  return {
    tileUrl,
    proxied: o.proxied === true,
    attribution: text(o.attribution) ?? "",
    attributionUrl: webLink(o.attribution_url),
  };
}

export async function loadMapConfig(fetchImpl: typeof fetch = apiFetch): Promise<MapConfig> {
  try {
    const res = await fetchImpl("/api/v1/map-config");
    if (!res.ok) return DEFAULT_MAP;
    return parseMapConfig(await res.json());
  } catch {
    return DEFAULT_MAP;
  }
}

/** The tile source from the server; null until it answered (the map waits for it). */
export function useMapConfig(): MapConfig | null {
  const [config, setConfig] = useState<MapConfig | null>(null);
  useEffect(() => {
    let live = true;
    void loadMapConfig().then((c) => {
      if (live) setConfig(c);
    });
    return () => {
      live = false;
    };
  }, []);
  return config;
}

function escapeHtml(s: string): string {
  return s.replace(/[&<>"']/g, (c) => `&#${c.charCodeAt(0)};`);
}

/** The attribution as the HTML Leaflet wants; text and link are escaped, the link opens safely. */
export function attributionHtml(c: MapConfig): string {
  if (c.attribution === "") return "";
  const label = escapeHtml(c.attribution);
  if (c.attributionUrl === null) return label;
  return `<a href="${escapeHtml(c.attributionUrl)}" target="_blank" rel="noopener noreferrer">${label}</a>`;
}

/** Host of the tile server, for messages. */
export function tileHost(tileUrl: string): string {
  try {
    return new URL(tileUrl.replace(/\{[a-z]\}/g, "0")).host;
  } catch {
    return tileUrl;
  }
}

/**
 * Judges the tiles of a map: a few failures before the first tile that loaded means the server
 * refuses us (policy block, offline, wrong URL). Later single failures are ordinary hiccups.
 */
export class TileWatch {
  private ok = 0;
  private failed = 0;
  constructor(private readonly threshold = 3) {}

  /** A tile loaded: the server serves us, whatever failed before. */
  loaded(): void {
    this.ok += 1;
  }

  /** A tile failed; returns true once the map should show the problem. */
  failure(): boolean {
    this.failed += 1;
    return this.ok === 0 && this.failed >= this.threshold;
  }
}
