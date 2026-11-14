// The AIS vessel map: ship symbols, labels and track lines on the shared base map
// (map/BaseMap.tsx, the same one the ADS-B and APRS views use). The vessels (last position, track,
// last heard) come from ais/model.ts.

import L from "leaflet";
import { useEffect, useRef, useState } from "react";
import { MapCanvas, useBaseMap } from "../map/BaseMap";
import type { MapConfig } from "../map/mapConfig";
import {
  GROUP_COLOR,
  MOORED_BELOW_KN,
  displayName,
  groupOf,
  pointing,
  positioned,
  vesselOpacity,
  type Vessel,
  type VesselGroup,
} from "./model";

interface Props {
  vessels: readonly Vessel[];
  selected: number | null;
  onSelect: (mmsi: number | null) => void;
  /** Increment to fit the map to all vessels. */
  fitSignal: number;
  /** Current time, for fading stale vessels. */
  nowMs: number;
  tiles: MapConfig;
}

interface Entry {
  marker: L.Marker;
  track: L.Polyline;
  iconKey: string;
  labelled: boolean;
}

/** Labels are drawn from this zoom on (a harbour has hundreds of ships). */
export const LABEL_ZOOM = 10;

const SHAPES: Record<VesselGroup | "moored", string> = {
  // An arrow-shaped hull, bow up (rotated to the heading).
  cargo: "M12 1.5 L18.5 21 L12 16.5 L5.5 21 Z",
  tanker: "M12 1.5 L18.5 21 L12 16.5 L5.5 21 Z",
  passenger: "M12 1.5 L18.5 21 L12 16.5 L5.5 21 Z",
  fishing: "M12 1.5 L18.5 21 L12 16.5 L5.5 21 Z",
  sailing: "M12 1.5 L18.5 21 L12 16.5 L5.5 21 Z",
  pleasure: "M12 1.5 L18.5 21 L12 16.5 L5.5 21 Z",
  tug: "M12 1.5 L18.5 21 L12 16.5 L5.5 21 Z",
  highspeed: "M12 1.5 L18.5 21 L12 16.5 L5.5 21 Z",
  special: "M12 1.5 L18.5 21 L12 16.5 L5.5 21 Z",
  other: "M12 1.5 L18.5 21 L12 16.5 L5.5 21 Z",
  // Shore and sea marks do not move: a square and a diamond.
  base: "M6 6 H18 V18 H6 Z",
  aton: "M12 3 L21 12 L12 21 L3 12 Z",
  // A small aircraft.
  sar: "M12 1 L14 9 L22 14 V16 L14 14 L13.2 20 L16 22 V23 L12 22 L8 23 V22 L10.8 20 L10 14 L2 16 V14 L10 9 Z",
  // A vessel at rest (or without a heading): a dot.
  moored: "M12 6.5 a5.5 5.5 0 1 0 0.001 0 Z",
};

/** The symbol of a vessel: an SVG in its group's colour, turned to where it points. */
function iconFor(v: Vessel, selected: boolean): { key: string; icon: L.DivIcon } {
  const group = groupOf(v);
  const direction = pointing(v);
  const fixed = group === "base" || group === "aton";
  const dot = !fixed && group !== "sar" && (direction === null || (v.sogKn ?? 0) < MOORED_BELOW_KN);
  const angle = fixed || dot || direction === null ? 0 : Math.round(direction / 5) * 5;
  const key = `${group}|${dot}|${angle}|${selected}`;
  const color = GROUP_COLOR[group];
  const shape = SHAPES[dot ? "moored" : group];
  const ring = selected ? '<span class="ais-ring"></span>' : "";
  const html =
    `${ring}<svg class="ais-svg" viewBox="0 0 24 24" width="24" height="24" aria-hidden="true" ` +
    `style="transform:rotate(${angle}deg)"><path d="${shape}" fill="${color}" /></svg>`;
  const icon = L.divIcon({
    className: "ais-icon",
    html,
    iconSize: [24, 24],
    iconAnchor: [12, 12],
  });
  return { key, icon };
}

export function AisMap({ vessels, selected, onSelect, fitSignal, nowMs, tiles }: Props) {
  const { hostRef, mapRef, tileProblem } = useBaseMap(tiles, () => onSelect(null));
  const entries = useRef(new Map<number, Entry>());
  const fitted = useRef(false);
  const onSelectRef = useRef(onSelect);
  const [labels, setLabels] = useState(false);
  useEffect(() => {
    onSelectRef.current = onSelect;
  });

  // Labels only when zoomed in.
  useEffect(() => {
    const m = mapRef.current;
    if (m === null) return;
    const update = () => setLabels(m.getZoom() >= LABEL_ZOOM);
    update();
    m.on("zoomend", update);
    return () => {
      m.off("zoomend", update);
    };
  }, [mapRef]);

  // Forget the markers with the map.
  useEffect(() => {
    const current = entries.current;
    return () => {
      current.clear();
      fitted.current = false;
    };
  }, []);

  // Vessels: add, move, fade, remove.
  useEffect(() => {
    const m = mapRef.current;
    if (m === null) return;
    const drawn = positioned(vessels);
    const live = new Set<number>();
    for (const v of drawn) {
      live.add(v.mmsi);
      const at: L.LatLngTuple = [v.lat, v.lon];
      const isSelected = v.mmsi === selected;
      const { key, icon } = iconFor(v, isSelected);
      const trackPoints = v.track.map((p): L.LatLngTuple => [p.lat, p.lon]);
      const wantLabel = labels || isSelected;
      let entry = entries.current.get(v.mmsi);
      if (entry === undefined) {
        const marker = L.marker(at, { icon, keyboard: false, riseOnHover: true })
          .on("click", (e) => {
            L.DomEvent.stopPropagation(e);
            onSelectRef.current(v.mmsi);
          })
          .addTo(m);
        const track = L.polyline(trackPoints, { weight: 2, interactive: false }).addTo(m);
        entry = { marker, track, iconKey: key, labelled: false };
        entries.current.set(v.mmsi, entry);
      } else {
        entry.marker.setLatLng(at);
        entry.track.setLatLngs(trackPoints);
        if (entry.iconKey !== key) {
          entry.marker.setIcon(icon);
          entry.iconKey = key;
        }
      }
      if (wantLabel) {
        const text = displayName(v);
        if (entry.labelled) entry.marker.setTooltipContent(text);
        else
          entry.marker.bindTooltip(text, {
            permanent: true,
            direction: "right",
            offset: [10, 0],
            className: "adsb-label",
          });
        entry.labelled = true;
      } else if (entry.labelled) {
        entry.marker.unbindTooltip();
        entry.labelled = false;
      }
      const opacity = vesselOpacity(v, nowMs);
      entry.marker.setOpacity(opacity);
      entry.marker.setZIndexOffset(isSelected ? 1000 : 0);
      entry.marker.getTooltip()?.setOpacity(opacity * 0.9);
      entry.track.setStyle({
        color: isSelected ? "#818cf8" : GROUP_COLOR[groupOf(v)],
        opacity: opacity * (isSelected ? 0.9 : 0.4),
        weight: isSelected ? 3 : 2,
      });
    }
    for (const [mmsi, entry] of entries.current) {
      if (live.has(mmsi)) continue;
      entry.marker.remove();
      entry.track.remove();
      entries.current.delete(mmsi);
    }
    // First time there is something to show: frame it (later the user owns the view).
    if (!fitted.current && drawn.length > 0) {
      fitAll(m, drawn);
      fitted.current = true;
    }
  }, [mapRef, vessels, selected, nowMs, labels]);

  // "Fit" button.
  useEffect(() => {
    const m = mapRef.current;
    if (m !== null && fitSignal > 0) fitAll(m, positioned(vessels));
    // Only the signal triggers a fit, not every new picture.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [fitSignal]);

  // A selection made in the list brings the vessel into view.
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
      label="AIS vessel map"
      stillShown="Vessels are still shown."
    />
  );
}

function fitAll(m: L.Map, vessels: readonly { lat: number; lon: number }[]): void {
  const points = vessels.map((s): L.LatLngTuple => [s.lat, s.lon]);
  const first = points[0];
  if (first === undefined) return;
  if (points.length === 1) {
    m.setView(first, 11);
    return;
  }
  m.fitBounds(L.latLngBounds(points), { padding: [40, 40], maxZoom: 12 });
}
