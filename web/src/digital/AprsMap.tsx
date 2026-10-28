// The APRS station map: markers and track lines on the shared base map (map/BaseMap.tsx). The
// stations (last position, track, last heard) come from digital/aprsStations.ts.

import L from "leaflet";
import { useEffect, useRef } from "react";
import { MapCanvas, useBaseMap } from "../map/BaseMap";
import type { MapConfig } from "../map/mapConfig";
import { aprsSymbolName } from "../decoders/render";
import { stationOpacity, type AprsStation } from "./aprsStations";

interface Props {
  stations: readonly AprsStation[];
  selected: string | null;
  onSelect: (id: string | null) => void;
  /** Increment to fit the map to all stations. */
  fitSignal: number;
  /** Current time, for fading stale stations. */
  nowMs: number;
  tiles: MapConfig;
}

interface Entry {
  marker: L.Marker;
  track: L.Polyline;
  iconKey: string;
}

/** The symbol table and code as a small badge: the code character on a coloured disc. */
function iconFor(s: AprsStation, selected: boolean): { key: string; icon: L.DivIcon } {
  const code = s.symbol?.[1] ?? "?";
  const key = `${code}|${s.live}|${selected}`;
  const ring = selected ? '<span class="aprs-ring"></span>' : "";
  const glyph = code.replace(/[&<>"']/g, (c) => `&#${c.charCodeAt(0)};`);
  const icon = L.divIcon({
    className: "aprs-icon",
    html: `${ring}<span class="aprs-badge${s.live ? "" : " aprs-badge-dead"}">${glyph}</span>`,
    iconSize: [24, 24],
    iconAnchor: [12, 12],
  });
  return { key, icon };
}

export function symbolTitle(s: AprsStation): string | null {
  return s.symbol === null ? null : aprsSymbolName(s.symbol);
}

export function AprsMap({ stations, selected, onSelect, fitSignal, nowMs, tiles }: Props) {
  const { hostRef, mapRef, tileProblem } = useBaseMap(tiles, () => onSelect(null));
  const entries = useRef(new Map<string, Entry>());
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
      fitted.current = false;
    };
  }, []);

  // Stations: add, move, fade, remove.
  useEffect(() => {
    const m = mapRef.current;
    if (m === null) return;
    const live = new Set<string>();
    for (const s of stations) {
      live.add(s.id);
      const at: L.LatLngTuple = [s.lat, s.lon];
      const isSelected = s.id === selected;
      const { key, icon } = iconFor(s, isSelected);
      const trackPoints = s.track.map((p): L.LatLngTuple => [p.lat, p.lon]);
      let entry = entries.current.get(s.id);
      if (entry === undefined) {
        const marker = L.marker(at, { icon, keyboard: false, riseOnHover: true })
          .bindTooltip(s.id, {
            permanent: true,
            direction: "right",
            offset: [10, 0],
            className: "adsb-label",
          })
          .on("click", (e) => {
            L.DomEvent.stopPropagation(e);
            onSelectRef.current(s.id);
          })
          .addTo(m);
        const track = L.polyline(trackPoints, { weight: 2, interactive: false }).addTo(m);
        entry = { marker, track, iconKey: key };
        entries.current.set(s.id, entry);
      } else {
        entry.marker.setLatLng(at);
        entry.track.setLatLngs(trackPoints);
        if (entry.iconKey !== key) {
          entry.marker.setIcon(icon);
          entry.iconKey = key;
        }
      }
      const opacity = stationOpacity(s, nowMs);
      entry.marker.setOpacity(opacity);
      entry.marker.setZIndexOffset(isSelected ? 1000 : 0);
      entry.marker.getTooltip()?.setOpacity(opacity * 0.9);
      entry.track.setStyle({
        color: isSelected ? "#818cf8" : "#f59e0b",
        opacity: opacity * (isSelected ? 0.9 : 0.6),
        weight: isSelected ? 3 : 2,
      });
    }
    for (const [id, entry] of entries.current) {
      if (live.has(id)) continue;
      entry.marker.remove();
      entry.track.remove();
      entries.current.delete(id);
    }
    // First time there is something to show: frame it (later the user owns the view).
    if (!fitted.current && stations.length > 0) {
      fitAll(m, stations);
      fitted.current = true;
    }
  }, [mapRef, stations, selected, nowMs]);

  // "Fit" button.
  useEffect(() => {
    const m = mapRef.current;
    if (m !== null && fitSignal > 0) fitAll(m, stations);
    // Only the signal triggers a fit, not every new picture.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [fitSignal]);

  // A selection made in the list brings the station into view.
  useEffect(() => {
    const m = mapRef.current;
    const entry = selected === null ? undefined : entries.current.get(selected);
    if (m === null || entry === undefined) return;
    const at = entry.marker.getLatLng();
    if (!m.getBounds().pad(-0.1).contains(at)) m.panTo(at);
  }, [mapRef, selected]);

  return (
    <MapCanvas
      hostRef={hostRef}
      tileProblem={tileProblem}
      tiles={tiles}
      label="APRS station map"
      stillShown="Stations are still shown."
    />
  );
}

function fitAll(m: L.Map, stations: readonly AprsStation[]): void {
  const points = stations.map((s): L.LatLngTuple => [s.lat, s.lon]);
  const first = points[0];
  if (first === undefined) return;
  if (points.length === 1) {
    m.setView(first, 10);
    return;
  }
  m.fitBounds(L.latLngBounds(points), { padding: [40, 40], maxZoom: 12 });
}
