import { lazy, Suspense, useEffect, useMemo, useState, useSyncExternalStore } from "react";
import type { Receiver } from "../shell/useReceiver";
import {
  Alert,
  Badge,
  Button,
  EmptyState,
  IconButton,
  PlainButton,
  SearchIcon,
  Toggle,
  ChevronDownIcon,
  CloseIcon,
  ExpandIcon,
  cn,
} from "../ui";
import { StateBadge } from "../plugins/parts";
import { loadMapConfig, type MapConfig } from "./mapConfig";
import {
  ALTITUDE_LEGEND,
  formatAge,
  formatAlt,
  formatRssi,
  formatSpeed,
  formatTrack,
  label,
  matchesFilter,
  sortAircraft,
  tracked,
  type SortDir,
  type SortKey,
  type Tracked,
} from "./model";

// Leaflet is a chunk of its own: it is only fetched when somebody opens the map.
const AdsbMap = lazy(() => import("./AdsbMap").then((m) => ({ default: m.AdsbMap })));

/** Re-render every second so aircraft keep ageing and fading between two status polls. */
function useNow(intervalMs = 1000): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), intervalMs);
    return () => clearInterval(id);
  }, [intervalMs]);
  return now;
}

/** The tile source from the server; null until it answered (the map waits for it). */
function useMapConfig(): MapConfig | null {
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

const COLUMNS: readonly { key: SortKey; title: string; align?: "right" }[] = [
  { key: "callsign", title: "Callsign" },
  { key: "icao", title: "ICAO" },
  { key: "alt", title: "Altitude", align: "right" },
  { key: "speed", title: "Speed", align: "right" },
  { key: "track", title: "Track", align: "right" },
  { key: "rssi", title: "Signal", align: "right" },
  { key: "age", title: "Heard", align: "right" },
];

/**
 * The ADS-B view: a full-window overlay with the aircraft map and a sortable table next to it.
 * Aircraft the plugin has not heard for 60 s are gone; before that they fade out.
 */
export function AdsbView({ r }: { r: Receiver }) {
  const snap = useSyncExternalStore(r.adsb.subscribe, r.adsb.getSnapshot);
  const now = useNow();
  const [query, setQuery] = useState("");
  const [onlyPositions, setOnlyPositions] = useState(false);
  const [sort, setSort] = useState<{ key: SortKey; dir: SortDir }>({ key: "age", dir: "asc" });
  const [selected, setSelected] = useState<string | null>(null);
  const [fitSignal, setFitSignal] = useState(0);
  const tiles = useMapConfig();

  const status = snap.reply?.status ?? null;
  const all = useMemo(
    () => (status === null ? [] : tracked(status, snap.receivedAt, now)),
    [status, snap.receivedAt, now],
  );
  const shown = useMemo(
    () =>
      sortAircraft(
        all.filter((a) => matchesFilter(a, query) && (!onlyPositions || a.lat !== null)),
        sort.key,
        sort.dir,
      ),
    [all, query, onlyPositions, sort],
  );
  const onMap = useMemo(() => all.filter((a) => a.lat !== null), [all]);
  const chosen = selected === null ? null : (all.find((a) => a.icao === selected) ?? null);

  const close = () => r.adsb.closeView();
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      const field = e.target instanceof HTMLInputElement;
      if (!field) r.adsb.closeView();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [r.adsb]);

  const toggleSort = (key: SortKey) =>
    setSort((s) =>
      s.key === key
        ? { key, dir: s.dir === "asc" ? "desc" : "asc" }
        : { key, dir: key === "callsign" || key === "icao" || key === "age" ? "asc" : "desc" },
    );

  return (
    <div
      role="dialog"
      aria-label="ADS-B aircraft"
      data-testid="adsb-view"
      className="fixed inset-0 z-40 flex flex-col bg-surface-0 text-fg"
    >
      <header className="flex shrink-0 flex-wrap items-center gap-x-3 gap-y-2 border-b border-border bg-surface-1 px-4 py-2">
        <h2 className="text-sm font-semibold">ADS-B aircraft</h2>
        {snap.reply !== null && <StateBadge state={snap.reply.state} />}
        {status !== null && (
          <>
            <Badge tone="accent" title="Aircraft heard in the last 60 seconds">
              {all.length} aircraft
            </Badge>
            <Badge title="Aircraft with a known position (on the map)">{onMap.length} on map</Badge>
            {status.messagesPerS !== null && (
              <Badge title="Messages decoded per second">
                {Math.round(status.messagesPerS)} msg/s
              </Badge>
            )}
          </>
        )}
        <span className="flex-1" />
        <label className="relative flex items-center">
          <span className="sr-only">Filter aircraft</span>
          <SearchIcon size={14} className="pointer-events-none absolute left-2.5 text-fg-subtle" />
          <input
            type="search"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Callsign, ICAO, squawk"
            className="focus-ring h-8 w-44 rounded-md border border-border bg-surface-0 pr-2 pl-8 text-sm placeholder:text-fg-subtle"
          />
        </label>
        <Toggle checked={onlyPositions} onChange={setOnlyPositions} label="Only with position" />
        <Button
          size="sm"
          variant="secondary"
          icon={<ExpandIcon />}
          onClick={() => setFitSignal((n) => n + 1)}
        >
          Fit map
        </Button>
        <IconButton label="Close the ADS-B view" onClick={close}>
          <CloseIcon />
        </IconButton>
      </header>

      {snap.error !== null && (
        <Alert tone="warn" className="m-3 mb-0">
          {snap.error}
        </Alert>
      )}
      {status?.error != null && <Alert className="m-3 mb-0">{status.error}</Alert>}

      {snap.instanceId === null ? (
        <div className="flex flex-1 items-center justify-center p-6">
          <EmptyState title="ADS-B is not running">
            Start it from the ADS-B tab to see aircraft here.
          </EmptyState>
        </div>
      ) : (
        <div className="flex min-h-0 flex-1 flex-col md:flex-row">
          <div className="relative min-h-0 flex-1">
            <Suspense
              fallback={
                <p role="status" className="p-4 text-sm text-fg-muted">
                  Loading the map…
                </p>
              }
            >
              {tiles !== null && (
                <AdsbMap
                  aircraft={onMap}
                  receiver={status?.receiver ?? null}
                  selected={selected}
                  onSelect={setSelected}
                  fitSignal={fitSignal}
                  tiles={tiles}
                />
              )}
            </Suspense>
            <Legend />
          </div>
          <section
            aria-label="Aircraft table"
            className="flex max-h-[45%] min-h-0 shrink-0 flex-col border-t border-border bg-surface-1 md:max-h-none md:w-[32rem] md:border-t-0 md:border-l"
          >
            {chosen !== null && <Details a={chosen} onClose={() => setSelected(null)} />}
            <div className="min-h-0 flex-1 overflow-auto">
              {shown.length === 0 ? (
                <p className="p-4 text-sm text-fg-muted" role="status">
                  {status === null || status.mode === "starting"
                    ? "Waiting for the decoder…"
                    : all.length === 0
                      ? "No aircraft heard yet. Check the antenna (1090 MHz) and the gain."
                      : "No aircraft match the filter."}
                </p>
              ) : (
                <table className="w-full text-left text-xs tabular-nums">
                  <thead className="sticky top-0 z-10 bg-surface-1 text-fg-muted shadow-[0_1px_0_var(--border)]">
                    <tr>
                      {COLUMNS.map((c) => (
                        <th
                          key={c.key}
                          scope="col"
                          aria-sort={
                            sort.key === c.key
                              ? sort.dir === "asc"
                                ? "ascending"
                                : "descending"
                              : "none"
                          }
                          className={cn(
                            "px-2 py-1.5 font-medium",
                            c.align === "right" && "text-right",
                          )}
                        >
                          <PlainButton
                            onClick={() => toggleSort(c.key)}
                            className={cn(
                              "inline-flex items-center gap-0.5 rounded px-0.5 hover:text-fg",
                              sort.key === c.key && "text-fg",
                            )}
                            aria-label={`Sort by ${c.title}`}
                          >
                            {c.title}
                            {sort.key === c.key && (
                              <ChevronDownIcon
                                size={12}
                                className={sort.dir === "asc" ? "rotate-180" : undefined}
                              />
                            )}
                          </PlainButton>
                        </th>
                      ))}
                    </tr>
                  </thead>
                  <tbody>
                    {shown.map((a) => (
                      <Row
                        key={a.icao}
                        a={a}
                        selected={a.icao === selected}
                        onSelect={() => setSelected(a.icao === selected ? null : a.icao)}
                      />
                    ))}
                  </tbody>
                </table>
              )}
            </div>
            {status?.truncated === true && (
              <p className="border-t border-border px-3 py-1.5 text-[11px] text-fg-muted">
                Showing the freshest {status.aircraft.length} of {status.total} aircraft.
              </p>
            )}
          </section>
        </div>
      )}
    </div>
  );
}

function Row({ a, selected, onSelect }: { a: Tracked; selected: boolean; onSelect: () => void }) {
  return (
    <tr
      data-testid={`adsb-row-${a.icao}`}
      data-selected={selected || undefined}
      aria-selected={selected}
      onClick={onSelect}
      style={{ opacity: a.opacity }}
      className={cn(
        "cursor-pointer border-t border-border/60 hover:bg-surface-2",
        selected && "bg-accent-soft hover:bg-accent-soft",
      )}
    >
      <td className="px-2 py-1.5 font-medium">
        <PlainButton onClick={onSelect} className="rounded px-0.5 text-left">
          {label(a)}
          {a.lat === null && <span className="sr-only"> (no position)</span>}
        </PlainButton>
        {a.lat === null && (
          <span aria-hidden="true" className="ml-1 text-fg-subtle" title="No position yet">
            ·
          </span>
        )}
      </td>
      <td className="px-2 py-1.5 font-mono text-fg-muted uppercase">{a.icao}</td>
      <td className="px-2 py-1.5 text-right">{formatAlt(a)}</td>
      <td className="px-2 py-1.5 text-right">{formatSpeed(a)}</td>
      <td className="px-2 py-1.5 text-right">{formatTrack(a)}</td>
      <td className="px-2 py-1.5 text-right text-fg-muted">{formatRssi(a)}</td>
      <td className="px-2 py-1.5 text-right text-fg-muted">{formatAge(a.ageS)}</td>
    </tr>
  );
}

function Details({ a, onClose }: { a: Tracked; onClose: () => void }) {
  const fields: [string, string][] = [
    ["ICAO", a.icao.toUpperCase()],
    ["Altitude", formatAlt(a)],
    ["Speed", formatSpeed(a)],
    ["Track", formatTrack(a)],
    ["Climb", a.vrateFpm === null ? "–" : `${a.vrateFpm > 0 ? "+" : ""}${a.vrateFpm} ft/min`],
    ["Squawk", a.squawk ?? "–"],
    ["Category", a.category ?? "–"],
    [
      "Position",
      a.lat === null || a.lon === null ? "–" : `${a.lat.toFixed(4)}, ${a.lon.toFixed(4)}`,
    ],
    ["Messages", String(a.messages)],
  ];
  return (
    <div data-testid="adsb-details" className="shrink-0 border-b border-border p-3">
      <div className="mb-2 flex items-center gap-2">
        <h3 className="text-sm font-semibold">{label(a)}</h3>
        <span className="flex-1" />
        <IconButton label="Clear selection" size="sm" onClick={onClose}>
          <CloseIcon />
        </IconButton>
      </div>
      <dl className="grid grid-cols-3 gap-x-3 gap-y-1.5 text-xs">
        {fields.map(([k, v]) => (
          <div key={k}>
            <dt className="text-[11px] text-fg-muted">{k}</dt>
            <dd className="font-medium tabular-nums">{v}</dd>
          </div>
        ))}
      </dl>
    </div>
  );
}

function Legend() {
  return (
    <ul
      aria-label="Altitude colours"
      className="pointer-events-none absolute bottom-6 left-3 z-[500] flex flex-wrap gap-x-3 gap-y-1 rounded-lg border border-border bg-surface-1/90 px-2.5 py-1.5 text-[11px] text-fg-muted shadow-pop backdrop-blur"
    >
      {ALTITUDE_LEGEND.map(([color, text]) => (
        <li key={text} className="flex items-center gap-1.5">
          <span aria-hidden="true" className="size-2 rounded-full" style={{ background: color }} />
          {text}
        </li>
      ))}
    </ul>
  );
}
