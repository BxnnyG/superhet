// The map every Superhet map view builds on (ADS-B aircraft, APRS stations): Leaflet
// (BSD-2-Clause, exact version pinned in package.json) on the tiles the server configured
// (map/mapConfig.ts). Imperative on purpose: Leaflet owns its DOM, React only hands it the
// current picture. A view calls `useBaseMap` for the map and the tile layer, draws its own
// markers on `mapRef.current` and renders `<MapCanvas>`.

import L from "leaflet";
import "leaflet/dist/leaflet.css";
import { useEffect, useRef, useState, type RefObject } from "react";
import { Alert } from "../ui";
import { attributionHtml, tileHost, TileWatch, type MapConfig } from "./mapConfig";

/**
 * Tile requests carry the page's origin as Referer. The page itself sends none cross-origin
 * (`Referrer-Policy: same-origin`), but OpenStreetMap's tile usage policy blocks requests
 * without a valid Referer (HTTP 403, "Access blocked"). Scoped to the tile images only; only
 * matters in direct tile mode, the default proxy serves tiles from our own origin.
 */
export const TILE_REFERRER_POLICY = "strict-origin-when-cross-origin";

/** Shown instead of the browser's broken-image icon when a tile fails (1x1, transparent). */
const BLANK_TILE = "data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7";

export interface BaseMap {
  hostRef: RefObject<HTMLDivElement | null>;
  mapRef: RefObject<L.Map | null>;
  /** The tile server refuses us or is unreachable. */
  tileProblem: boolean;
}

/**
 * Creates the Leaflet map once and keeps its tile layer in step with `tiles`. Call this before
 * the view's own effects: they run later and find `mapRef.current` set.
 */
export function useBaseMap(tiles: MapConfig, onBackgroundClick?: () => void): BaseMap {
  const hostRef = useRef<HTMLDivElement>(null);
  const mapRef = useRef<L.Map | null>(null);
  const [tileProblem, setTileProblem] = useState(false);
  const clickRef = useRef(onBackgroundClick);
  useEffect(() => {
    clickRef.current = onBackgroundClick;
  });

  useEffect(() => {
    const el = hostRef.current;
    if (el === null) return;
    const m = L.map(el, { center: [50, 10], zoom: 5, worldCopyJump: true });
    m.on("click", () => clickRef.current?.());
    mapRef.current = m;
    return () => {
      m.remove();
      mapRef.current = null;
    };
  }, []);

  // The tile layer: a different source replaces it.
  useEffect(() => {
    const m = mapRef.current;
    if (m === null) return;
    const watch = new TileWatch();
    const layer = L.tileLayer(tiles.tileUrl, {
      maxZoom: 19,
      attribution: attributionHtml(tiles),
      referrerPolicy: TILE_REFERRER_POLICY,
      errorTileUrl: BLANK_TILE,
    });
    layer.on("tileload", () => {
      watch.loaded();
      setTileProblem(false);
    });
    layer.on("tileerror", () => {
      if (watch.failure()) setTileProblem(true);
    });
    layer.addTo(m);
    return () => {
      layer.remove();
    };
  }, [tiles]);

  return { hostRef, mapRef, tileProblem };
}

interface CanvasProps {
  hostRef: BaseMap["hostRef"];
  tileProblem: boolean;
  tiles: MapConfig;
  label: string;
  /** What is still shown when the background is missing ("Aircraft are still shown."). */
  stillShown: string;
  /** Extra class on the map element (view-specific styling). */
  className?: string;
}

export function MapCanvas({
  hostRef,
  tileProblem,
  tiles,
  label,
  stillShown,
  className,
}: CanvasProps) {
  return (
    <div className="relative size-full">
      <div
        ref={hostRef}
        role="application"
        aria-label={label}
        className={`sh-map ${className ?? ""} size-full bg-surface-1`}
      />
      {tileProblem && (
        <div className="pointer-events-none absolute inset-x-0 top-0 z-[1000] p-3">
          <Alert tone="warn" className="pointer-events-auto mx-auto max-w-lg">
            {tiles.proxied
              ? "The server could not load the map background from its tile server. "
              : `The map background could not be loaded from ${tileHost(tiles.tileUrl)}. `}
            The tile server may refuse this server (OpenStreetMap blocks clients that break its tile
            usage policy) or it is not reachable. {stillShown} An administrator can set{" "}
            <code>SUPERHET_MAP_TILE_URL</code> to another tile server.
          </Alert>
        </div>
      )}
    </div>
  );
}
