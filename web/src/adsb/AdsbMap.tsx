// The aircraft map: markers and trails on the shared base map (map/BaseMap.tsx).
// The positions and fading come from `Tracked` (adsb/model.ts); trails are kept here, in the
// browser, for as long as an aircraft stays on the picture.

import L from "leaflet";
import { useEffect, useRef } from "react";
import { MapCanvas, useBaseMap } from "../map/BaseMap";
import type { MapConfig } from "../map/mapConfig";
import { altitudeColor, label, type Tracked } from "./model";
import type { SymbolKind } from "./icao";

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
  /** Symbol per aircraft (type from the database or the emitter category); default: a jet. */
  symbols?: ReadonlyMap<string, SymbolKind>;
  /** Coverage outline (one point per bearing sector) to draw around the receiver; null = none. */
  coverage?: readonly (readonly [number, number])[] | null;
}

const STROKE =
  'fill="currentColor" stroke="rgba(0,0,0,.65)" stroke-width="1.1" stroke-linejoin="round"';

/** Symbols pointing north (up) on a 24 x 24 grid; they are rotated to the track. */
const SHAPES: Record<SymbolKind, string> = {
  jet: `<path d="M12 1.5l2.3 7.2 8.2 4.8v2.2l-8.2-2.4-.5 5.4 3 2.1v1.7L12 21.4l-4.8 1.1v-1.7l3-2.1-.5-5.4-8.2 2.4v-2.2l8.2-4.8z" ${STROKE}/>`,
  heavy: `<path d="M12 0.8l2.2 6.4 9.3 6.2v2.6l-9.3-2.6-.4 4.6 3.1 2.2v2l-4.9-.9-4.9.9v-2l3.1-2.2-.4-4.6-9.3 2.6v-2.6l9.3-6.2z" ${STROKE}/>`,
  prop: `<path d="M12 3l1.3 5.2 9.2.3v2.6l-9.2.4-.5 6.2 2.9 1.6v1.9L12 20.9l-3.7.3v-1.9l2.9-1.6-.5-6.2-9.2-.4V8.5l9.2-.3z" ${STROKE}/><path d="M9.5 2.5h5" stroke="rgba(0,0,0,.65)" stroke-width="1.6" stroke-linecap="round"/>`,
  heli: `<circle cx="12" cy="12" r="3.6" ${STROKE}/><path d="M12 15.4V22M9 22h6M3 12h18M12 3v18" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" fill="none"/>`,
  glider: `<path d="M12 4l1 7 10.5 2.2v1.4L13 14.4l-.6 6 2.4 1.4v1H9.2v-1l2.4-1.4-.6-6L0.5 14.6v-1.4L11 11z" ${STROKE}/>`,
  balloon: `<circle cx="12" cy="9" r="6.5" ${STROKE}/><rect x="9.5" y="17.5" width="5" height="4" rx=".8" ${STROKE}/>`,
  uav: `<path d="M5 5l14 14M19 5L5 19" stroke="rgba(0,0,0,.65)" stroke-width="3.2" stroke-linecap="round"/><path d="M5 5l14 14M19 5L5 19" stroke="currentColor" stroke-width="1.6" stroke-linecap="round"/><circle cx="12" cy="12" r="2.6" ${STROKE}/>`,
  ground: `<rect x="6" y="5" width="12" height="14" rx="2" ${STROKE}/>`,
  unknown: `<path d="M12 1.5l2.3 7.2 8.2 4.8v2.2l-8.2-2.4-.5 5.4 3 2.1v1.7L12 21.4l-4.8 1.1v-1.7l3-2.1-.5-5.4-8.2 2.4v-2.2l8.2-4.8z" ${STROKE}/>`,
};

const svgOf = (kind: SymbolKind): string =>
  `<svg viewBox="0 0 24 24" width="26" height="26" aria-hidden="true">${SHAPES[kind]}</svg>`;

function iconFor(
  a: Tracked,
  selected: boolean,
  kind: SymbolKind,
): { key: string; icon: L.DivIcon } {
  // Balloons and ground vehicles have no heading worth showing.
  const rotation =
    kind === "balloon" || kind === "ground" ? 0 : Math.round((a.trackDeg ?? 0) / 5) * 5;
  const color = altitudeColor(a);
  const key = `${rotation}|${color}|${selected}|${a.trackDeg === null}|${kind}`;
  const ring = selected ? '<span class="adsb-ring"></span>' : "";
  const icon = L.divIcon({
    className: "adsb-icon",
    html: `${ring}<span class="adsb-plane" data-kind="${kind}" style="color:${color};transform:rotate(${rotation}deg)">${svgOf(kind)}</span>`,
    iconSize: [28, 28],
    iconAnchor: [14, 14],
  });
  return { key, icon };
}

export function AdsbMap({
  aircraft,
  receiver,
  selected,
  onSelect,
  fitSignal,
  tiles,
  symbols,
  coverage = null,
}: Props) {
  const { hostRef, mapRef: map, tileProblem } = useBaseMap(tiles, () => onSelect(null));
  const entries = useRef(new Map<string, Entry>());
  const station = useRef<L.CircleMarker | null>(null);
  const fitted = useRef(false);
  const onSelectRef = useRef(onSelect);
  useEffect(() => {
    onSelectRef.current = onSelect;
  });

  // Forget the markers with the map.
  useEffect(() => {
    const current = entries.current;
    return () => {
      current.clear();
      station.current = null;
      fitted.current = false;
    };
  }, []);

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
  }, [map, receiver]);

  // Coverage outline: the farthest reception per sector, drawn around the receiver.
  const outline = useRef<L.Polygon | null>(null);
  useEffect(() => {
    const m = map.current;
    if (m === null) return;
    outline.current?.remove();
    outline.current = null;
    if (coverage === null || coverage.length < 3) return;
    outline.current = L.polygon(
      coverage.map(([lat, lon]) => [lat, lon] as L.LatLngTuple),
      {
        color: "#818cf8",
        weight: 1.5,
        fillColor: "#818cf8",
        fillOpacity: 0.08,
        interactive: false,
        className: "adsb-coverage",
      },
    ).addTo(m);
    return () => {
      outline.current?.remove();
      outline.current = null;
    };
  }, [map, coverage]);

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
      const { key, icon } = iconFor(a, isSelected, symbols?.get(a.icao) ?? "jet");
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
  }, [map, aircraft, selected, receiver, symbols]);

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
  }, [map, selected]);

  return (
    <MapCanvas
      hostRef={hostRef}
      tileProblem={tileProblem}
      tiles={tiles}
      label="Aircraft map"
      stillShown="Aircraft are still shown."
      className="adsb-map"
    />
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
