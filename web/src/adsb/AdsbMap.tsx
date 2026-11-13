// The aircraft map: markers, trails, labels and range rings on the shared base map
// (map/BaseMap.tsx). Imperative on purpose: a marker is a DOM element that is created once and then
// only touched where something changed (position, rotation, colour, label text), so 200+ aircraft
// cost a few attribute writes per second and no React render. Trails are drawn on one canvas
// (TrailCanvas.ts). The positions and fading come from `Tracked` (adsb/model.ts); the trail
// points come from the store (adsb/trails.ts).

import L from "leaflet";
import { useEffect, useRef } from "react";
import { MapCanvas, useBaseMap } from "../map/BaseMap";
import type { MapConfig } from "../map/mapConfig";
import { altitudeColor, emergencyOf, formatAlt, formatSpeed, label, type Tracked } from "./model";
import { DEFAULT_PREFS, type LabelFields } from "./prefs";
import { destination } from "./range";
import { SYMBOL_SIZE, symbolSvg, UPRIGHT, type SymbolKind } from "./symbols";
import { TrailCanvas, type TrailItem } from "./TrailCanvas";
import { trailSegments, type TrailStore } from "./trails";
import { distanceUnitName, kmPerDistanceUnit, type UnitSystem } from "./units";

/** Labels appear from this zoom on (a continent view would be a wall of text). */
export const LABEL_MIN_ZOOM = 6;
/** Pixel cell of the label declutter grid. */
const CELL_W = 56;
const CELL_H = 15;

interface Entry {
  marker: L.Marker;
  /** The marker's own elements, set once the marker is on the map. */
  plane: HTMLElement | null;
  tag: HTMLElement | null;
  line1: HTMLElement | null;
  line2: HTMLElement | null;
  ring: HTMLElement | null;
  kind: SymbolKind;
  /** Accumulated rotation (never wraps, so the symbol turns the short way). */
  turn: number;
  heading: number | null;
  color: string;
  opacity: number;
  at: L.LatLngTuple;
  text1: string;
  text2: string;
  title: string;
  /** The label would collide with another one and is hidden. */
  hidden: boolean;
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
  /** Symbol per aircraft (type from the database or the emitter category); default: unknown. */
  symbols?: ReadonlyMap<string, SymbolKind>;
  /** Coverage outline (one point per bearing sector) to draw around the receiver; null = none. */
  coverage?: readonly (readonly [number, number])[] | null;
  units?: UnitSystem;
  /** Where the trails are kept; none = no trails. */
  trails?: TrailStore;
  /** Trail length in minutes; 0 = none. */
  trailMinutes?: number;
  labels?: LabelFields;
  /** Range rings around the receiver, in the chosen distance unit. */
  rings?: boolean;
  /** Keep the selected aircraft in the middle of the map. */
  follow?: boolean;
  /** The user dragged the map: following ends. */
  onUserPan?: () => void;
}

function buildIcon(kind: SymbolKind): L.DivIcon {
  const size = SYMBOL_SIZE[kind];
  return L.divIcon({
    className: "adsb-icon",
    html:
      `<span class="adsb-ring" hidden></span>` +
      `<span class="adsb-plane" data-kind="${kind}">${symbolSvg(kind)}</span>` +
      `<span class="adsb-tag" hidden><span class="adsb-l1"></span><span class="adsb-l2"></span></span>`,
    iconSize: [size, size],
    iconAnchor: [size / 2, size / 2],
  });
}

function bind(entry: Entry): void {
  const el = entry.marker.getElement();
  entry.plane = el?.querySelector<HTMLElement>(".adsb-plane") ?? null;
  entry.tag = el?.querySelector<HTMLElement>(".adsb-tag") ?? null;
  entry.line1 = el?.querySelector<HTMLElement>(".adsb-l1") ?? null;
  entry.line2 = el?.querySelector<HTMLElement>(".adsb-l2") ?? null;
  entry.ring = el?.querySelector<HTMLElement>(".adsb-ring") ?? null;
  // The state of a fresh element is unknown: write everything once.
  entry.turn = Number.NaN;
  entry.color = "";
  entry.text1 = "\u0000";
  entry.text2 = "\u0000";
  entry.title = "";
  entry.opacity = -1;
  entry.hidden = true;
}

/** Nearest angle equivalent to `to` (mod 360) around `from`, so a turn takes the short way. */
function unwrap(from: number, to: number): number {
  if (Number.isNaN(from)) return to;
  return from + ((((to - from) % 360) + 540) % 360) - 180;
}

function labelTexts(a: Tracked, fields: LabelFields, units: UnitSystem): [string, string] {
  const l1 = fields.callsign ? label(a) : "";
  const second: string[] = [];
  if (fields.altitude) second.push(formatAlt(a, units).text);
  if (fields.speed && a.speedKt !== null) second.push(formatSpeed(a.speedKt, units).text);
  return [l1, second.join(" ")];
}

function titleOf(a: Tracked, units: UnitSystem): string {
  const alt = formatAlt(a, units);
  const parts = [label(a)];
  if (alt.text !== "–") parts.push(alt.other === "" ? alt.text : `${alt.text} (${alt.other})`);
  if (a.speedKt !== null) {
    const speed = formatSpeed(a.speedKt, units);
    parts.push(`${speed.text} (${speed.other})`);
  }
  return parts.join(" · ");
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
  units = "metric",
  trails,
  trailMinutes = 0,
  labels = DEFAULT_PREFS.labels,
  rings = false,
  follow = false,
  onUserPan,
}: Props) {
  const { hostRef, mapRef: map, tileProblem } = useBaseMap(tiles, () => onSelect(null));
  const entries = useRef(new Map<string, Entry>());
  const station = useRef<L.CircleMarker | null>(null);
  const trailCanvas = useRef<TrailCanvas | null>(null);
  const fitted = useRef(false);
  const onSelectRef = useRef(onSelect);
  const onPanRef = useRef(onUserPan);
  useEffect(() => {
    onSelectRef.current = onSelect;
    onPanRef.current = onUserPan;
  });

  // Forget the markers and the trail canvas with the map.
  useEffect(() => {
    const m = map.current;
    const current = entries.current;
    if (m !== null) {
      trailCanvas.current = new TrailCanvas(m);
      const onDrag = (): void => onPanRef.current?.();
      m.on("dragstart", onDrag);
      return () => {
        m.off("dragstart", onDrag);
        trailCanvas.current?.destroy();
        trailCanvas.current = null;
        current.clear();
        station.current = null;
        fitted.current = false;
      };
    }
    return () => {
      current.clear();
    };
  }, [map]);

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

  // Range rings around the receiver in the chosen unit, spaced to the zoom.
  useEffect(() => {
    const m = map.current;
    if (m === null || !rings || receiver === null) return;
    const group = L.layerGroup().addTo(m);
    const draw = (): void => {
      group.clearLayers();
      const unitKm = kmPerDistanceUnit(units);
      const mPerPx = (156543.03392 * Math.cos((receiver.lat * Math.PI) / 180)) / 2 ** m.getZoom();
      const stepUnits = niceStep((mPerPx * 140) / 1000 / unitKm);
      const size = m.getSize();
      const reach = (Math.hypot(size.x, size.y) * mPerPx) / 1000 / unitKm;
      const count = Math.min(12, Math.max(1, Math.ceil(reach / stepUnits)));
      for (let i = 1; i <= count; i++) {
        const km = i * stepUnits * unitKm;
        L.circle([receiver.lat, receiver.lon], {
          radius: km * 1000,
          fill: false,
          weight: 1,
          dashArray: "4 5",
          color: "#818cf8",
          opacity: 0.7,
          interactive: false,
          className: "adsb-range-ring",
        }).addTo(group);
        const [lat, lon] = destination(receiver, 0, km);
        L.marker([lat, lon], {
          interactive: false,
          keyboard: false,
          icon: L.divIcon({
            className: "adsb-ring-label",
            html: `${i * stepUnits} ${distanceUnitName(units)}`,
            iconSize: [0, 0],
          }),
        }).addTo(group);
      }
    };
    draw();
    m.on("zoomend resize", draw);
    return () => {
      m.off("zoomend resize", draw);
      group.remove();
    };
  }, [map, rings, receiver, units]);

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

  // Hide labels that would sit on top of another one (selected aircraft always keep theirs).
  const declutter = useRef<() => void>(() => {});
  useEffect(() => {
    declutter.current = () => {
      const m = map.current;
      if (m === null) return;
      const show = m.getZoom() >= LABEL_MIN_ZOOM || selected !== null;
      const taken = new Set<string>();
      // Selected first, then low aircraft (more interesting near an airport), then the rest.
      const order = [...entries.current.entries()].sort(([ia], [ib]) =>
        ia === selected ? -1 : ib === selected ? 1 : 0,
      );
      for (const [icao, entry] of order) {
        const tag = entry.tag;
        if (tag === null) continue;
        const empty = entry.text1 === "" && entry.text2 === "";
        let visible = !empty && (show || icao === selected);
        if (visible && icao !== selected) {
          const p = m.latLngToContainerPoint(entry.at);
          const w = Math.max(entry.text1.length, entry.text2.length) * 6 + 14;
          const x0 = Math.floor(p.x / CELL_W);
          const x1 = Math.floor((p.x + w) / CELL_W);
          const y0 = Math.floor((p.y - CELL_H / 2) / CELL_H);
          const y1 = Math.floor((p.y + CELL_H * (entry.text2 === "" ? 0.5 : 1.5)) / CELL_H);
          const cells: string[] = [];
          for (let x = x0; x <= x1; x++) for (let y = y0; y <= y1; y++) cells.push(`${x},${y}`);
          if (cells.some((c) => taken.has(c))) visible = false;
          else for (const c of cells) taken.add(c);
        }
        if (entry.hidden === visible) {
          entry.hidden = !visible;
          tag.hidden = !visible;
        }
      }
    };
  });
  useEffect(() => {
    const m = map.current;
    if (m === null) return;
    const run = (): void => declutter.current();
    m.on("zoomend moveend", run);
    return () => {
      m.off("zoomend moveend", run);
    };
  }, [map]);

  // Aircraft: add, move, rotate, recolour, relabel, fade, remove. Only what changed is written.
  useEffect(() => {
    const m = map.current;
    if (m === null) return;
    const live = new Set<string>();
    for (const a of aircraft) {
      if (a.lat === null || a.lon === null) continue;
      live.add(a.icao);
      const at: L.LatLngTuple = [a.lat, a.lon];
      const kind = symbols?.get(a.icao) ?? "unknown";
      let entry = entries.current.get(a.icao);
      if (entry !== undefined && entry.kind !== kind) {
        // A different symbol needs a different element (and size).
        entry.marker.setIcon(buildIcon(kind));
        entry.kind = kind;
        bind(entry);
      }
      if (entry === undefined) {
        const marker = L.marker(at, { icon: buildIcon(kind), keyboard: false, riseOnHover: true })
          .on("click", (e) => {
            L.DomEvent.stopPropagation(e);
            onSelectRef.current(a.icao);
          })
          .addTo(m);
        entry = {
          marker,
          plane: null,
          tag: null,
          line1: null,
          line2: null,
          ring: null,
          kind,
          turn: Number.NaN,
          heading: null,
          color: "",
          opacity: -1,
          at,
          text1: "",
          text2: "",
          title: "",
          hidden: true,
        };
        bind(entry);
        entries.current.set(a.icao, entry);
      }
      if (entry.at[0] !== at[0] || entry.at[1] !== at[1]) {
        entry.marker.setLatLng(at);
        entry.at = at;
      }
      const isSelected = a.icao === selected;
      // The symbol points along the track (the heading when there is no track).
      const course = a.trackDeg ?? a.headingDeg;
      if (entry.plane !== null) {
        const upright = UPRIGHT.has(kind) || course === null;
        const next = upright ? 0 : Math.round(course / 5) * 5;
        if (Number.isNaN(entry.turn) || next !== ((entry.turn % 360) + 360) % 360) {
          entry.turn = unwrap(entry.turn, next);
          entry.plane.style.transform = `rotate(${entry.turn}deg)`;
        }
        const color = altitudeColor(a);
        if (entry.color !== color) {
          entry.color = color;
          entry.plane.style.color = color;
        }
      }
      if (entry.ring !== null) {
        // The ring marks the selection and, in red, an emergency squawk (7500, 7600, 7700).
        const emergency = emergencyOf(a) !== null;
        entry.ring.hidden = !(isSelected || emergency);
        if (emergency) entry.ring.dataset.emergency = "true";
        else delete entry.ring.dataset.emergency;
      }
      const [t1, t2] = labelTexts(a, labels, units);
      if (entry.line1 !== null && entry.text1 !== t1) {
        entry.text1 = t1;
        entry.line1.textContent = t1;
      }
      if (entry.line2 !== null && entry.text2 !== t2) {
        entry.text2 = t2;
        entry.line2.textContent = t2;
      }
      if (entry.plane !== null) {
        const title = titleOf(a, units);
        if (entry.title !== title) {
          entry.title = title;
          entry.plane.title = title;
        }
      }
      if (entry.opacity !== a.opacity) {
        entry.opacity = a.opacity;
        entry.marker.setOpacity(a.opacity);
      }
      entry.marker.setZIndexOffset(isSelected ? 1000 : Math.round(SYMBOL_SIZE[kind] * 2));
    }
    for (const [icao, entry] of entries.current) {
      if (live.has(icao)) continue;
      entry.marker.remove();
      entries.current.delete(icao);
    }
    declutter.current();

    // Trails of the aircraft on the map, cut to the chosen length.
    const now = Date.now();
    const windowMs = trailMinutes * 60_000;
    const items: TrailItem[] = [];
    if (trails !== undefined && windowMs > 0) {
      for (const a of aircraft) {
        if (a.lat === null || a.lon === null) continue;
        const head = {
          lat: a.lat,
          lon: a.lon,
          altFt: a.altFt,
          onGround: a.onGround,
          t: now - a.ageS * 1000,
        };
        items.push({
          segments: trailSegments(trails.get(a.icao), now, windowMs, head),
          opacity: a.opacity,
          selected: a.icao === selected,
        });
      }
    }
    trailCanvas.current?.set(items);

    // Following keeps the selected aircraft in view without taking the map away from a drag.
    const chosen = follow && selected !== null ? entries.current.get(selected) : undefined;
    if (chosen !== undefined && live.has(selected ?? "")) {
      m.panTo(chosen.at, { animate: true, duration: 0.9, easeLinearity: 1, noMoveStart: true });
    }
    // First time there is something to show: frame it (later the user owns the view).
    if (!fitted.current && (live.size > 0 || receiver !== null)) {
      fitAll(m, aircraft, receiver);
      fitted.current = true;
    }
  }, [map, aircraft, selected, receiver, symbols, labels, units, trails, trailMinutes, follow]);

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

/** A round ring spacing at or above `wanted` (in display units): 1, 2, 5, 10, 20, 25, 50, 100... */
export function niceStep(wanted: number): number {
  const steps = [1, 2, 5, 10, 20, 25, 50, 100, 200, 250, 500, 1000, 2500];
  return steps.find((s) => s >= wanted) ?? 5000;
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
