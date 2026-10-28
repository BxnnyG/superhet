// The "Map" view of the APRS output: stations on the map, a list beside it, details of the
// selected station. Lazy (digital/DigitalDock.tsx) so Leaflet only loads when the map is opened.

import { lazy, Suspense, useEffect, useMemo, useState } from "react";
import type { DecoderEvent } from "../decoders/events";
import { formatCoords, mapLink } from "../decoders/render";
import { useMapConfig } from "../map/mapConfig";
import { Button, CloseIcon, IconButton, PlainButton, cn } from "../ui";
import { aprsStations, heardAgo, stationOpacity, type AprsStation } from "./aprsStations";

const AprsMap = lazy(() => import("./AprsMap").then((m) => ({ default: m.AprsMap })));

/** How often "heard 3 min ago" and the fading are refreshed. */
const TICK_MS = 15_000;

export function AprsMapView({ events }: { events: readonly DecoderEvent[] }) {
  const tiles = useMapConfig();
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), TICK_MS);
    return () => clearInterval(id);
  }, []);
  const [selected, setSelected] = useState<string | null>(null);
  const [fitSignal, setFitSignal] = useState(0);
  // The event log changes with every packet; `now` only moves the fading along.
  const stations = useMemo(() => aprsStations(events, now), [events, now]);
  const chosen = stations.find((s) => s.id === selected) ?? null;

  return (
    <div
      className="flex h-[22rem] max-h-full min-h-48 flex-1 flex-col md:flex-row"
      data-testid="aprs-map-view"
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
            <AprsMap
              stations={stations}
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
        {stations.length === 0 && (
          <p
            role="status"
            className="pointer-events-none absolute inset-x-0 bottom-6 z-[1000] mx-auto w-fit rounded-md border border-border bg-surface-1/90 px-3 py-1.5 text-sm text-fg-muted"
          >
            No station with a position heard yet.
          </p>
        )}
      </div>
      <section
        aria-label="APRS stations"
        className="flex max-h-[45%] min-h-0 shrink-0 flex-col border-t border-border bg-surface-1 md:max-h-none md:w-80 md:border-t-0 md:border-l"
      >
        {chosen !== null && <Details s={chosen} nowMs={now} onClose={() => setSelected(null)} />}
        <ul className="min-h-0 flex-1 overflow-y-auto">
          {stations.map((s) => (
            <li key={s.id}>
              <PlainButton
                aria-pressed={s.id === selected}
                onClick={() => setSelected(s.id === selected ? null : s.id)}
                className={cn(
                  "focus-ring flex w-full items-baseline gap-2 border-b border-border/60 px-3 py-1.5 text-left text-sm hover:bg-surface-2",
                  s.id === selected && "bg-accent-soft",
                )}
                style={{ opacity: stationOpacity(s, now) }}
              >
                <span className="num font-medium text-fg">{s.id}</span>
                <span className="ml-auto text-xs text-fg-subtle">{heardAgo(s, now)}</span>
              </PlainButton>
            </li>
          ))}
        </ul>
      </section>
    </div>
  );
}

function Details({ s, nowMs, onClose }: { s: AprsStation; nowMs: number; onClose: () => void }) {
  const coords = formatCoords(s.lat, s.lon);
  const link = mapLink(s.lat, s.lon);
  const rows: [string, string | null][] = [
    ["Heard", `${heardAgo(s, nowMs)} · ${s.packets} packet${s.packets === 1 ? "" : "s"}`],
    ["Position", coords],
    ["Symbol", s.symbol],
    ["Sent by", s.via],
    ["Speed", s.speedKn === null ? null : `${s.speedKn} kn`],
    ["Course", s.course === null ? null : `${s.course}°`],
    ["Altitude", s.altitudeM === null ? null : `${s.altitudeM} m`],
    ["Via", s.path.length === 0 ? null : s.path.join(", ")],
    ["Comment", s.comment],
    ["Track", s.track.length > 1 ? `${s.track.length} positions` : null],
  ];
  return (
    <div className="border-b border-border px-3 py-2" data-testid="aprs-details">
      <div className="flex items-center gap-2">
        <h3 className="num text-sm font-semibold text-fg">{s.id}</h3>
        {!s.live && <span className="text-xs text-fg-subtle">(removed)</span>}
        <IconButton label="Close details" size="sm" className="ml-auto" onClick={onClose}>
          <CloseIcon size={12} />
        </IconButton>
      </div>
      <dl className="mt-1 grid grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-0.5 text-xs">
        {rows.map(([k, v]) =>
          v === null ? null : (
            <div key={k} className="contents">
              <dt className="text-fg-subtle">{k}</dt>
              <dd className="break-words text-fg">{v}</dd>
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
