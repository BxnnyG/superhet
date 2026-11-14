// The "Map" view of the AIS output: vessels on the map, a searchable list beside it, details of the
// selected vessel. Lazy (digital/DigitalDock.tsx) so Leaflet only loads when the map is opened.

import { lazy, Suspense, useEffect, useMemo, useState } from "react";
import type { DecoderEvent } from "../decoders/events";
import { formatCoords, mapLink } from "../decoders/render";
import { useMapConfig } from "../map/mapConfig";
import { Button, CloseIcon, IconButton, PlainButton, SearchIcon, cn } from "../ui";
import {
  GROUP_COLOR,
  GROUP_LABEL,
  displayName,
  formatSize,
  formatSpeed,
  groupOf,
  heardAgo,
  matches,
  positioned,
  vesselOpacity,
  vessels as buildVessels,
  type Vessel,
} from "./model";

const AisMap = lazy(() => import("./AisMap").then((m) => ({ default: m.AisMap })));

/** How often "heard 3 min ago" and the fading are refreshed. */
const TICK_MS = 15_000;

export function AisMapView({ events }: { events: readonly DecoderEvent[] }) {
  const tiles = useMapConfig();
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), TICK_MS);
    return () => clearInterval(id);
  }, []);
  const [selected, setSelected] = useState<number | null>(null);
  const [fitSignal, setFitSignal] = useState(0);
  const [query, setQuery] = useState("");
  // The event log changes with every report; `now` only moves the fading along.
  const all = useMemo(() => buildVessels(events, now), [events, now]);
  const shown = useMemo(() => all.filter((v) => matches(v, query)), [all, query]);
  const onMap = positioned(shown).length;
  const chosen = all.find((v) => v.mmsi === selected) ?? null;

  return (
    <div
      className="flex h-[24rem] max-h-full min-h-48 flex-1 flex-col md:flex-row"
      data-testid="ais-map-view"
    >
      <div className="relative min-h-0 flex-1">
        <Suspense
          fallback={
            <p role="status" className="p-4 text-sm text-fg-muted">
              Loading the map…
            </p>
          }
        >
          {tiles !== null && (
            <AisMap
              vessels={shown}
              selected={selected}
              onSelect={setSelected}
              fitSignal={fitSignal}
              nowMs={now}
              tiles={tiles}
            />
          )}
        </Suspense>
        <div className="absolute top-2 right-2 z-[1000]">
          <Button size="sm" variant="secondary" onClick={() => setFitSignal((n) => n + 1)}>
            Fit
          </Button>
        </div>
        {onMap === 0 && (
          <p
            role="status"
            className="pointer-events-none absolute inset-x-0 bottom-6 z-[1000] mx-auto w-fit rounded-md border border-border bg-surface-1/90 px-3 py-1.5 text-sm text-fg-muted"
          >
            {all.length === 0
              ? "No vessel with a position heard yet."
              : "No vessel matches (or none has sent a position)."}
          </p>
        )}
      </div>
      <section
        aria-label="AIS vessels"
        className="flex max-h-[45%] min-h-0 shrink-0 flex-col border-t border-border bg-surface-1 md:max-h-none md:w-80 md:border-t-0 md:border-l"
      >
        {chosen !== null && <Details v={chosen} nowMs={now} onClose={() => setSelected(null)} />}
        <div className="border-b border-border px-3 py-1.5">
          <label className="relative flex items-center">
            <span className="sr-only">Search vessels</span>
            <SearchIcon
              size={14}
              className="pointer-events-none absolute left-2.5 text-fg-subtle"
            />
            <input
              type="search"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder="Name, MMSI, callsign, destination"
              className="focus-ring h-8 w-full rounded-md border border-border bg-surface-0 pr-2 pl-8 text-sm placeholder:text-fg-subtle"
            />
          </label>
        </div>
        <ul className="min-h-0 flex-1 overflow-y-auto">
          {shown.map((v) => (
            <li key={v.mmsi}>
              <PlainButton
                aria-pressed={v.mmsi === selected}
                onClick={() => setSelected(v.mmsi === selected ? null : v.mmsi)}
                className={cn(
                  "focus-ring flex w-full items-baseline gap-2 border-b border-border/60 px-3 py-1.5 text-left text-sm hover:bg-surface-2",
                  v.mmsi === selected && "bg-accent-soft",
                )}
                style={{ opacity: vesselOpacity(v, now) }}
              >
                <span
                  aria-hidden="true"
                  className="size-2 shrink-0 self-center rounded-full"
                  style={{ background: GROUP_COLOR[groupOf(v)] }}
                />
                <span className="min-w-0 truncate font-medium text-fg">{displayName(v)}</span>
                <span className="num ml-auto shrink-0 text-xs text-fg-subtle">
                  {v.sogKn === null ? "" : `${v.sogKn.toFixed(1)} kn`}
                </span>
              </PlainButton>
            </li>
          ))}
        </ul>
      </section>
    </div>
  );
}

function Details({ v, nowMs, onClose }: { v: Vessel; nowMs: number; onClose: () => void }) {
  const coords = v.lat !== null && v.lon !== null ? formatCoords(v.lat, v.lon) : null;
  const link = v.lat !== null && v.lon !== null ? mapLink(v.lat, v.lon) : null;
  const group = groupOf(v);
  const rows: [string, string | null][] = [
    ["MMSI", String(v.mmsi)],
    ["Callsign", v.callsign],
    ["IMO", v.imo === null ? null : String(v.imo)],
    ["Type", v.typeText ?? GROUP_LABEL[group]],
    ["Status", v.statusText],
    ["Speed", formatSpeed(v.sogKn)],
    ["Course", v.cogDeg === null ? null : `${Math.round(v.cogDeg)}°`],
    ["Heading", v.headingDeg === null ? null : `${v.headingDeg}°`],
    ["Altitude", v.altitudeM === null ? null : `${v.altitudeM} m`],
    ["Destination", v.destination],
    ["ETA (UTC)", v.eta],
    ["Draught", v.draughtM === null ? null : `${v.draughtM} m`],
    ["Size", formatSize(v)],
    ["Position", coords],
    ["Heard", `${heardAgo(v, nowMs)} · ${v.reports} report${v.reports === 1 ? "" : "s"}`],
    ["Track", v.track.length > 1 ? `${v.track.length} positions` : null],
  ];
  return (
    <div className="border-b border-border px-3 py-2" data-testid="ais-details">
      <div className="flex items-center gap-2">
        <span
          aria-hidden="true"
          className="size-2.5 shrink-0 rounded-full"
          style={{ background: GROUP_COLOR[group] }}
        />
        <h3 className="min-w-0 truncate text-sm font-semibold text-fg">{displayName(v)}</h3>
        <IconButton label="Close details" size="sm" className="ml-auto" onClick={onClose}>
          <CloseIcon size={12} />
        </IconButton>
      </div>
      <dl className="mt-1 grid grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-0.5 text-xs">
        {rows.map(([k, val]) =>
          val === null ? null : (
            <div key={k} className="contents">
              <dt className="text-fg-subtle">{k}</dt>
              <dd className="break-words text-fg">{val}</dd>
            </div>
          ),
        )}
      </dl>
      {link !== null && (
        <a
          href={link.href}
          target="_blank"
          rel="noopener noreferrer"
          className="focus-ring mt-1 inline-block text-xs text-accent hover:underline"
        >
          {link.label}
        </a>
      )}
    </div>
  );
}
