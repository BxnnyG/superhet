// The aircraft map: Leaflet (BSD-2-Clause, exact version pinned in package.json) on OpenStreetMap
// tiles (or the server the admin configured, adsb/mapConfig.ts). Imperative on purpose: Leaflet
// owns its DOM, React only hands it the current picture.
// The positions and fading come from `Tracked` (adsb/model.ts); trails are kept here, in the
// browser, for as long as an aircraft stays on the picture.

import L from "leaflet";
import "leaflet/dist/leaflet.css";
import { useEffect, useRef, useState } from "react";
import { Alert } from "../ui";
import { attributionHtml, tileHost, TileWatch, type MapConfig } from "./mapConfig";
import { altitudeColor, label, type Tracked } from "./model";

/**
 * Tile requests carry the page's origin as Referer. The page itself sends none cross-origin
 * (`Referrer-Policy: same-origin`), but OpenStreetMap's tile usage policy blocks requests
 * without a valid Referer (HTTP 403, "Access blocked"). Scoped to the tile images only.
 */
export const TILE_REFERRER_POLICY = "strict-origin-when-cross-origin";

/** Shown instead of the browser's broken-image icon when a tile fails (1x1, transparent). */
const BLANK_TILE = "data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7";

/** Positions kept per aircraft for its trail. */
const TRAIL_POINTS = 40;

interface Entry {
  marker: L.Marker;
  trail: L.Polyline;
  points: L.LatLngTuple[];
  iconKey: string;
}

interface Props {
  aircraft: readonly Tracked[];
  receiver: { lat: number; lon: number } | null;
  selected: string | null;
  onSelect: (icao: string | null) => void;
  /** Increment to fit the map to everything on it. */
  fitSignal: number;
  /** Where the tiles come from. */
  tiles: MapConfig;
}

const PLANE =
  '<svg viewBox="0 0 24 24" width="26" height="26" aria-hidden="true"><path d="M12 1.5l2.3 7.2 8.2 4.8v2.2l-8.2-2.4-.5 5.4 3 2.1v1.7L12 21.4l-4.8 1.1v-1.7l3-2.1-.5-5.4-8.2 2.4v-2.2l8.2-4.8z" fill="currentColor" stroke="rgba(0,0,0,.65)" stroke-width="1.1" stroke-linejoin="round"/></svg>';

function iconFor(a: Tracked, selected: boolean): { key: string; icon: L.DivIcon } {
  const rotation = Math.round((a.trackDeg ?? 0) / 5) * 5;
  const color = altitudeColor(a);
  const key = `${rotation}|${color}|${selected}|${a.trackDeg === null}`;
  const ring = selected ? '<span class="adsb-ring"></span>' : "";
  const icon = L.divIcon({
    className: "adsb-icon",
    html: `${ring}<span class="adsb-plane" style="color:${color};transform:rotate(${rotation}deg)">${PLANE}</span>`,
    iconSize: [28, 28],
    iconAnchor: [14, 14],
  });
  return { key, icon };
}

export function AdsbMap({ aircraft, receiver, selected, onSelect, fitSignal, tiles }: Props) {
  const host = useRef<HTMLDivElement>(null);
  const map = useRef<L.Map | null>(null);
  const entries = useRef(new Map<string, Entry>());
  const station = useRef<L.CircleMarker | null>(null);
  const fitted = useRef(false);
  const [tileProblem, setTileProblem] = useState(false);
  const onSelectRef = useRef(onSelect);
  useEffect(() => {
    onSelectRef.current = onSelect;
  });

  // Create the map once.
  useEffect(() => {
    const el = host.current;
    if (el === null) return;
    const m = L.map(el, { center: [50, 10], zoom: 5, worldCopyJump: true });
    m.on("click", () => onSelectRef.current(null));
    map.current = m;
    const current = entries.current;
    return () => {
      m.remove();
      map.current = null;
      current.clear();
      station.current = null;
      fitted.current = false;
    };
  }, []);

  // The tile layer: a different source replaces it.
  useEffect(() => {
    const m = map.current;
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

  // The station marker.
  useEffect(() => {
    const m = map.current;
    if (m === null) return;
    station.current?.remove();
    station.current = null;
    if (receiver === null) return;
    station.current = L.circleMarker([receiver.lat, receiver.lon], {
      radius: 6,
      color: "#818cf8",
      weight: 2,
      fillColor: "#818cf8",
      fillOpacity: 0.35,
    })
      .bindTooltip("Receiver", { direction: "top" })
      .addTo(m);
  }, [receiver]);

  // Aircraft: add, move, recolour, fade, remove.
  useEffect(() => {
    const m = map.current;
    if (m === null) return;
    const live = new Set<string>();
    for (const a of aircraft) {
      if (a.lat === null || a.lon === null) continue;
      live.add(a.icao);
      const at: L.LatLngTuple = [a.lat, a.lon];
      const isSelected = a.icao === selected;
      const { key, icon } = iconFor(a, isSelected);
      let entry = entries.current.get(a.icao);
      if (entry === undefined) {
        const marker = L.marker(at, { icon, keyboard: false, riseOnHover: true })
          .bindTooltip(label(a), {
            permanent: true,
            direction: "right",
            offset: [10, 0],
            className: "adsb-label",
          })
          .on("click", (e) => {
            L.DomEvent.stopPropagation(e);
            onSelectRef.current(a.icao);
          })
          .addTo(m);
        const trail = L.polyline([at], { weight: 1.5, interactive: false }).addTo(m);
        entry = { marker, trail, points: [at], iconKey: key };
        entries.current.set(a.icao, entry);
      } else {
        const last = entry.points[entry.points.length - 1];
        if (last === undefined || last[0] !== at[0] || last[1] !== at[1]) {
          entry.points.push(at);
          if (entry.points.length > TRAIL_POINTS) entry.points.shift();
          entry.trail.setLatLngs(entry.points);
          entry.marker.setLatLng(at);
        }
        if (entry.iconKey !== key) {
          entry.marker.setIcon(icon);
          entry.iconKey = key;
        }
        entry.marker.setTooltipContent(label(a));
      }
      entry.marker.setOpacity(a.opacity);
      entry.marker.setZIndexOffset(isSelected ? 1000 : 0);
      entry.marker.getTooltip()?.setOpacity(a.opacity * 0.9);
      entry.trail.setStyle({ color: altitudeColor(a), opacity: a.opacity * 0.55 });
    }
    for (const [icao, entry] of entries.current) {
      if (live.has(icao)) continue;
      entry.marker.remove();
      entry.trail.remove();
      entries.current.delete(icao);
    }
    // First time there is something to show: frame it (later the user owns the view).
    if (!fitted.current && (live.size > 0 || receiver !== null)) {
      fitAll(m, aircraft, receiver);
      fitted.current = true;
    }
  }, [aircraft, selected, receiver]);

  // "Fit" button.
  useEffect(() => {
    const m = map.current;
    if (m !== null && fitSignal > 0) fitAll(m, aircraft, receiver);
    // Only the signal triggers a fit, not every new picture.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [fitSignal]);

  // A selection made in the table brings the aircraft into view.
  useEffect(() => {
    const m = map.current;
    const entry = selected === null ? undefined : entries.current.get(selected);
    if (m === null || entry === undefined) return;
    const at = entry.marker.getLatLng();
    if (!m.getBounds().pad(-0.1).contains(at)) m.panTo(at);
  }, [selected]);

  return (
    <div className="relative size-full">
      <div
        ref={host}
        role="application"
        aria-label="Aircraft map"
        className="adsb-map size-full bg-surface-1"
      />
      {tileProblem && (
        <div className="pointer-events-none absolute inset-x-0 top-0 z-[1000] p-3">
          <Alert tone="warn" className="pointer-events-auto mx-auto max-w-lg">
            The map background could not be loaded from {tileHost(tiles.tileUrl)}. The tile server
            may refuse this server (OpenStreetMap blocks clients that break its tile usage policy)
            or it is not reachable. Aircraft are still shown. An administrator can set{" "}
            <code>SUPERHET_MAP_TILE_URL</code> to another tile server.
          </Alert>
        </div>
      )}
    </div>
  );
}

function fitAll(
  m: L.Map,
  aircraft: readonly Tracked[],
  receiver: { lat: number; lon: number } | null,
): void {
  const points: L.LatLngTuple[] = aircraft.flatMap((a) =>
    a.lat !== null && a.lon !== null ? [[a.lat, a.lon] as L.LatLngTuple] : [],
  );
  if (receiver !== null) points.push([receiver.lat, receiver.lon]);
  if (points.length === 0) return;
  if (points.length === 1 && points[0]) {
    m.setView(points[0], 8);
    return;
  }
  m.fitBounds(L.latLngBounds(points), { padding: [40, 40], maxZoom: 10 });
}
