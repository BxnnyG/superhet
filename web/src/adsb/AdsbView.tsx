import { lazy, Suspense, useEffect, useMemo, useState, useSyncExternalStore } from "react";
import { usePermissions } from "../auth/context";
import type { Receiver } from "../shell/useReceiver";
import {
  Alert,
  Badge,
  Button,
  EmptyState,
  IconButton,
  PlainButton,
  SearchIcon,
  SegmentedControl,
  Toggle,
  ChevronDownIcon,
  CloseIcon,
  ExpandIcon,
  cn,
} from "../ui";
import { StateBadge } from "../plugins/parts";
import { useMapConfig } from "../map/mapConfig";
import { countryOf, flagEmoji, operatorCode, photoLinks, symbolKind, SYMBOL_LABEL } from "./icao";
import type { DatabaseStatus, InfoCache } from "./info";
import { RangePanel } from "./RangePanel";
import { outline, sectorMax } from "./range";
import { useRange } from "./useRange";
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

const COLUMNS: readonly { key: SortKey | "type"; title: string; align?: "right" }[] = [
  { key: "callsign", title: "Callsign" },
  { key: "icao", title: "ICAO" },
  { key: "type", title: "Type" },
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
  const [side, setSide] = useState<"aircraft" | "range">("aircraft");
  const [hours, setHours] = useState(24);
  const [showOutline, setShowOutline] = useState(false);
  const tiles = useMapConfig();
  const perms = usePermissions();
  const infoVersion = useSyncExternalStore(r.adsb.info.subscribe, r.adsb.info.getSnapshot);
  const [db, setDb] = useState<DatabaseStatus | null>(null);
  useEffect(() => {
    const ctrl = new AbortController();
    r.adsb.client.database(ctrl.signal).then(
      (status) => {
        if (!ctrl.signal.aborted) setDb(status);
      },
      () => {
        // The note is optional; the table works without it.
      },
    );
    return () => ctrl.abort();
  }, [r.adsb.client]);

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

  // Registration, type and airline from the local aircraft database, asked for in batches.
  const wanted = useMemo(
    () => ({
      icaos: all.map((a) => a.icao).sort(),
      operators: [...new Set(all.map((a) => operatorCode(a.callsign)).filter((c) => c !== null))],
    }),
    [all],
  );
  const wantedKey = `${wanted.icaos.join(",")}|${wanted.operators.join(",")}`;
  useEffect(() => {
    r.adsb.info.request(wanted.icaos, wanted.operators);
    // The key stands for the lists; a new array with the same content asks for nothing new.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [r.adsb.info, wantedKey]);
  const symbols = useMemo(() => {
    void infoVersion;
    return new Map(
      all.map((a) => {
        const info = r.adsb.info.aircraftInfo(a.icao);
        return [
          a.icao,
          symbolKind(info?.typeClass ?? null, info?.wake ?? null, a.category),
        ] as const;
      }),
    );
  }, [all, r.adsb.info, infoVersion]);

  // Range statistics: the coverage outline on the map and the Range tab.
  const device = snap.reply?.device ?? r.device;
  const range = useRange(r.adsb.client, device, hours, snap.instanceId !== null);
  const coverage = useMemo(
    () =>
      showOutline && range.data?.receiver
        ? outline(range.data.receiver, sectorMax(range.data.cells, null))
        : null,
    [showOutline, range.data],
  );
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
                  symbols={symbols}
                  coverage={coverage}
                />
              )}
            </Suspense>
            <Legend />
          </div>
          <section
            aria-label="Aircraft table"
            className="flex max-h-[45%] min-h-0 shrink-0 flex-col border-t border-border bg-surface-1 md:max-h-none md:w-[38rem] md:border-t-0 md:border-l"
          >
            <div className="shrink-0 border-b border-border px-3 py-2">
              <SegmentedControl
                aria-label="Side panel"
                size="sm"
                value={side}
                onChange={setSide}
                options={[
                  { value: "aircraft", label: "Aircraft" },
                  { value: "range", label: "Range" },
                ]}
              />
            </div>
            {side === "range" ? (
              <RangePanel
                api={r.adsb.client}
                device={device}
                range={range}
                hours={hours}
                onHours={setHours}
                canReset={perms.canUsePlugins}
                showOutline={showOutline}
                onShowOutline={setShowOutline}
              />
            ) : (
              <>
                {chosen !== null && (
                  <Details
                    a={chosen}
                    onClose={() => setSelected(null)}
                    info={r.adsb.info}
                    version={infoVersion}
                  />
                )}
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
                              {c.key === "type" ? (
                                c.title
                              ) : (
                                <PlainButton
                                  onClick={() => toggleSort(c.key as SortKey)}
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
                              )}
                            </th>
                          ))}
                        </tr>
                      </thead>
                      <tbody>
                        {shown.map((a) => (
                          <Row
                            key={a.icao}
                            a={a}
                            info={r.adsb.info}
                            version={infoVersion}
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
                <DatabaseNote db={db} />
              </>
            )}
          </section>
        </div>
      )}
    </div>
  );
}

function Flag({ icao }: { icao: string }) {
  const country = countryOf(icao);
  if (country === null || country.code === "") return null;
  return (
    <span title={country.name} data-testid={`adsb-flag-${icao}`} className="mr-1">
      <span aria-hidden="true">{flagEmoji(country.code)}</span>
      <span className="sr-only">{country.name}</span>
    </span>
  );
}

function Row({
  a,
  selected,
  onSelect,
  info,
}: {
  a: Tracked;
  selected: boolean;
  onSelect: () => void;
  info: InfoCache;
  /** Re-renders the row when lookups arrive. */
  version: number;
}) {
  const known = info.aircraftInfo(a.icao);
  const operator = info.operatorInfo(operatorCode(a.callsign));
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
        {operator !== null && (
          <div className="max-w-[9rem] truncate text-[11px] font-normal text-fg-muted">
            {operator.name}
          </div>
        )}
      </td>
      <td className="px-2 py-1.5 font-mono text-fg-muted">
        <Flag icao={a.icao} />
        <span className="uppercase">{a.icao}</span>
        {known?.registration != null && (
          <div className="text-[11px] text-fg">{known.registration}</div>
        )}
      </td>
      <td className="px-2 py-1.5 text-fg-muted" title={known?.typeDesc ?? undefined}>
        {known?.typeCode ?? "–"}
      </td>
      <td className="px-2 py-1.5 text-right">{formatAlt(a)}</td>
      <td className="px-2 py-1.5 text-right">{formatSpeed(a)}</td>
      <td className="px-2 py-1.5 text-right">{formatTrack(a)}</td>
      <td className="px-2 py-1.5 text-right text-fg-muted">{formatRssi(a)}</td>
      <td className="px-2 py-1.5 text-right text-fg-muted">{formatAge(a.ageS)}</td>
    </tr>
  );
}

function Details({
  a,
  onClose,
  info,
}: {
  a: Tracked;
  onClose: () => void;
  info: InfoCache;
  /** Re-renders the panel when lookups arrive. */
  version: number;
}) {
  const known = info.aircraftInfo(a.icao);
  const operator = info.operatorInfo(operatorCode(a.callsign));
  const country = countryOf(a.icao);
  const kind = symbolKind(known?.typeClass ?? null, known?.wake ?? null, a.category);
  const links = photoLinks(a.icao, known?.registration ?? null);
  const fields: [string, string][] = [
    ["ICAO", a.icao.toUpperCase()],
    ["Registration", known?.registration ?? "–"],
    [
      "Type",
      known === null ? "–" : [known.typeCode, known.typeDesc].filter(Boolean).join(" ") || "–",
    ],
    ["Operator", operator?.name ?? "–"],
    ["Country", country === null ? "–" : `${flagEmoji(country.code)} ${country.name}`.trim()],
    ["Symbol", SYMBOL_LABEL[kind]],
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
        {known?.military === true && <Badge tone="warn">military</Badge>}
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
      {links.length > 0 && (
        <p className="mt-2 text-xs text-fg-muted">
          Photos:{" "}
          {links.map((l, i) => (
            <span key={l.label}>
              {i > 0 && ", "}
              <a
                href={l.url}
                target="_blank"
                rel="noopener noreferrer"
                className="text-accent underline-offset-2 hover:underline"
              >
                {l.label}
              </a>
            </span>
          ))}{" "}
          (opens the site; nothing is loaded from it here)
        </p>
      )}
    </div>
  );
}

/** Source and licence of the aircraft database (ODC-By asks for attribution where it is used). */
function DatabaseNote({ db }: { db: DatabaseStatus | null }) {
  if (db === null) return null;
  const d = db.database;
  return (
    <p
      data-testid="adsb-db-note"
      className="border-t border-border px-3 py-1.5 text-[11px] text-fg-muted"
    >
      {d === null ? (
        "No aircraft database imported: registration, type and airline are not shown. An admin can import one under Admin, ADS-B data."
      ) : (
        <>
          Aircraft data:{" "}
          <a href={d.sourceUrl} target="_blank" rel="noopener noreferrer" className="underline">
            {d.source}
          </a>
          , licence{" "}
          <a href={d.licenceUrl} target="_blank" rel="noopener noreferrer" className="underline">
            {d.licence}
          </a>
          . Flags from the ICAO address block.
        </>
      )}
    </p>
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
