import { useEffect, useLayoutEffect, useMemo, useRef, useState, useSyncExternalStore } from "react";
import { usePermissions } from "../auth/context";
import { ActionsMenu } from "../bookmarks/ActionsMenu";
import { layoutRows, visibleWindow } from "../bookmarks/virtual";
import type { AnnotationTarget } from "../spectrum/annotations";
import { pluginName } from "../plugins/store";
import type { PluginManifest } from "../plugins/api";
import {
  Alert,
  Badge,
  Button,
  Chip,
  ChevronRightIcon,
  ConfirmDialog,
  Dialog,
  EmptyState,
  IconButton,
  MoreIcon,
  PauseIcon,
  PlainButton,
  SearchIcon,
  Select,
  cn,
  toasts,
  type BadgeTone,
} from "../ui";
import { vfoColor } from "../vfo/model";
import { downloadEventLog } from "./api";
import {
  CORE_SOURCE,
  KIND_CHIPS,
  KIND_HINT,
  KIND_LABEL,
  eventNote,
  eventTarget,
  formatFullTime,
  formatMhz,
  formatTime,
  type CoreKind,
  type DecoderEvent,
} from "./events";
import { COMPACT_ROW_H, describeEvent, isCoreEvent, rowHeight, type EventView } from "./render";
import type { DecoderEventStore } from "./store";
/** Viewport height assumed until the list has been measured (and in jsdom). */
const FALLBACK_VIEWPORT_H = 360;
const SEARCH_DEBOUNCE_MS = 250;

const KIND_TONE: Record<CoreKind, BadgeTone> = {
  dtmf: "accent",
  ctcss: "success",
  dcs: "success",
  rds_ps: "neutral",
  rds_rt: "neutral",
  rds_pi: "neutral",
};

interface Props {
  store: DecoderEventStore;
  device: string;
  /** Installed plugins (operators); names for the source filter. Empty for other roles. */
  plugins?: readonly PluginManifest[];
  onTune: (target: AnnotationTarget) => void;
}

/**
 * The decoder event log: filter chips, search, pause, export and clear in a pinned header, then a
 * windowed table (fixed row height, newest first). Fills its container; only the table scrolls.
 * A click on a row tunes to the frequency the event was heard on.
 */
export function EventLog({ store, device, plugins = [], onTune }: Props) {
  const snap = useSyncExternalStore(store.subscribe, store.getSnapshot);
  const { events, filter } = snap;
  const [query, setQuery] = useState(filter.q);
  const [confirmClear, setConfirmClear] = useState(false);
  const [details, setDetails] = useState<DecoderEvent | null>(null);
  // Clearing the shared history needs the operator role.
  const canClear = usePermissions().canClearHistory;
  const [busy, setBusy] = useState(false);

  // Search runs on the server; wait until typing pauses.
  useEffect(() => {
    if (query.trim() === filter.q.trim()) return;
    const t = setTimeout(() => store.setFilter({ ...filter, q: query }), SEARCH_DEBOUNCE_MS);
    return () => clearTimeout(t);
  }, [query, filter, store]);

  const activeKinds = filter.kinds;
  const toggleChip = (kinds: readonly string[]) => {
    const on = kinds.every((k) => activeKinds.includes(k));
    const next = on
      ? activeKinds.filter((k) => !kinds.includes(k))
      : [...activeKinds, ...kinds.filter((k) => !activeKinds.includes(k))];
    store.setFilter({ ...filter, kinds: next });
  };

  // Sources: the built-in detectors plus every plugin that is installed or has events in the list.
  const sourceIds = useMemo(() => {
    const ids = new Set<string>(plugins.map((p) => p.id));
    for (const e of events) if (e.source !== CORE_SOURCE) ids.add(e.source);
    if (filter.source !== null && filter.source !== CORE_SOURCE) ids.add(filter.source);
    return [...ids].sort();
  }, [plugins, events, filter.source]);
  const sourceOptions = useMemo(
    () => [
      { value: "", label: "All sources" },
      { value: CORE_SOURCE, label: "Built-in detectors" },
      ...sourceIds.map((id) => ({ value: id, label: pluginName(plugins, id) })),
    ],
    [sourceIds, plugins],
  );
  // Kind chips: the built-in kinds, or the event types a chosen plugin has produced.
  const pluginKinds = useMemo(() => {
    if (filter.source === null || filter.source === CORE_SOURCE) return [];
    const kinds = new Set<string>(filter.kinds);
    for (const e of events) if (e.source === filter.source) kinds.add(e.kind);
    return [...kinds].sort();
  }, [events, filter.source, filter.kinds]);

  // Windowing.
  const heights = useMemo(() => events.map(rowHeight), [events]);
  const layout = useMemo(() => layoutRows(heights), [heights]);
  const scroller = useRef<HTMLDivElement>(null);
  const [scrollTop, setScrollTop] = useState(0);
  const [viewportH, setViewportH] = useState(FALLBACK_VIEWPORT_H);
  useEffect(() => {
    const el = scroller.current;
    if (!el || typeof ResizeObserver === "undefined") return;
    const measure = () => setViewportH(el.clientHeight || FALLBACK_VIEWPORT_H);
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(el);
    return () => observer.disconnect();
  }, []);
  const win = visibleWindow(layout, heights, scrollTop, viewportH);

  // New rows arrive at the top: while scrolled down keep the same rows in view.
  const firstId = useRef<number | null>(null);
  useLayoutEffect(() => {
    const el = scroller.current;
    const prev = firstId.current;
    firstId.current = events[0]?.id ?? null;
    if (!el || prev === null || el.scrollTop <= 0) return;
    const at = events.findIndex((e) => e.id === prev);
    if (at > 0) el.scrollTop += heights.slice(0, at).reduce((a, b) => a + b, 0);
  }, [events, heights]);

  // Reaching the bottom loads the next older page.
  const nearEnd = win.end >= events.length - 10;
  useEffect(() => {
    if (nearEnd && snap.next !== null && !snap.loading) void store.loadMore();
  }, [nearEnd, snap.next, snap.loading, store]);

  const clear = async () => {
    setBusy(true);
    try {
      const n = await store.clear();
      toasts.push({
        tone: "success",
        title: n === 1 ? "1 event deleted" : `${n} events deleted`,
        key: "decoder-events-clear",
        durationMs: 3000,
      });
      setConfirmClear(false);
    } catch (e) {
      toasts.push({
        tone: "warn",
        title: "Could not clear the history",
        body: e instanceof Error ? e.message : String(e),
        key: "decoder-events-clear",
      });
    } finally {
      setBusy(false);
    }
  };

  const download = () => downloadEventLog(device, filter);

  const filtering = filter.kinds.length > 0 || filter.source !== null || filter.q.trim() !== "";
  return (
    <div className="flex min-h-0 flex-1 flex-col" role="region" aria-label="Decoder event log">
      <div className="flex shrink-0 flex-col gap-2 border-b border-border px-3 pb-2">
        <div className="flex items-center gap-1">
          <div className="relative min-w-0 flex-1">
            <SearchIcon
              size={14}
              className="pointer-events-none absolute top-1/2 left-2 -translate-y-1/2 text-fg-subtle"
            />
            <input
              type="search"
              aria-label="Search decoder events"
              placeholder="Search value…"
              value={query}
              autoComplete="off"
              onChange={(e) => setQuery(e.target.value)}
              className="focus-ring h-8 w-full rounded-md border border-border-strong bg-surface-1 pr-2 pl-7 text-sm text-fg placeholder:text-fg-subtle"
            />
          </div>
          <Button
            size="sm"
            variant={snap.paused ? "primary" : "secondary"}
            icon={snap.paused ? undefined : <PauseIcon size={12} />}
            aria-pressed={snap.paused}
            title={
              snap.paused
                ? "Show the events that arrived while paused"
                : "Freeze the list; new events are held back"
            }
            onClick={() => (snap.paused ? store.resume() : store.pause())}
          >
            {snap.paused
              ? `Resume${snap.pending.length > 0 ? ` (${snap.pending.length})` : ""}`
              : "Pause"}
          </Button>
          <ActionsMenu
            label="More log actions"
            icon={<MoreIcon />}
            actions={[
              { label: "Export CSV", onSelect: download },
              {
                label: "Clear history…",
                separated: true,
                disabled: !canClear,
                onSelect: () => setConfirmClear(true),
              },
            ]}
          />
        </div>
        <div className="flex flex-wrap items-center gap-1" role="group" aria-label="Event kinds">
          {sourceIds.length > 0 && (
            <Select
              aria-label="Source"
              size="sm"
              className="max-w-44"
              options={sourceOptions}
              value={filter.source ?? ""}
              onChange={(v) =>
                store.setFilter({ ...filter, source: v === "" ? null : v, kinds: [] })
              }
            />
          )}
          {pluginKinds.length > 0
            ? pluginKinds.map((kind) => (
                <Chip
                  key={kind}
                  selected={activeKinds.includes(kind)}
                  title={`Event type ${kind}`}
                  className="h-7"
                  onClick={() => toggleChip([kind])}
                >
                  {kind}
                </Chip>
              ))
            : (filter.source === null || filter.source === CORE_SOURCE) &&
              KIND_CHIPS.map((chip) => (
                <Chip
                  key={chip.label}
                  selected={chip.kinds.every((k) => activeKinds.includes(k))}
                  title={chip.title}
                  className="h-7"
                  onClick={() => toggleChip(chip.kinds)}
                >
                  {chip.label}
                </Chip>
              ))}
        </div>
      </div>

      <div
        ref={scroller}
        data-testid="event-scroller"
        className="min-h-0 flex-1 overflow-y-auto"
        onScroll={(e) => setScrollTop(e.currentTarget.scrollTop)}
      >
        {snap.error !== null && (
          <div className="p-3">
            <Alert tone="warn">
              <p>The event history is unavailable: {snap.error}</p>
              <Button size="sm" className="mt-2" onClick={() => store.refresh()}>
                Retry
              </Button>
            </Alert>
          </div>
        )}
        {!snap.loaded && snap.error === null && (
          <p className="p-3 text-xs text-fg-subtle">Loading…</p>
        )}
        {snap.loaded && snap.error === null && events.length === 0 && (
          <div className="p-3">
            <EmptyState title={filtering ? "No matching events" : "Nothing heard yet"}>
              {filtering
                ? "Change or clear the filter to see more."
                : "DTMF digits, CTCSS tones, RDS changes and the messages of decoder plugins are logged here as they are decoded."}
            </EmptyState>
          </div>
        )}
        {events.length > 0 && (
          <div role="list" aria-label="Decoder events">
            <div style={{ height: layout.offsets[win.start] ?? 0 }} aria-hidden="true" />
            {events.slice(win.start, win.end).map((e, i) => (
              <Row
                key={e.id}
                event={e}
                index={win.start + i}
                onTune={onTune}
                onDetails={setDetails}
              />
            ))}
            <div
              style={{ height: layout.total - (layout.offsets[win.end] ?? layout.total) }}
              aria-hidden="true"
            />
          </div>
        )}
        {snap.loading && events.length > 0 && (
          <p className="p-2 text-center text-xs text-fg-subtle">Loading older events…</p>
        )}
      </div>

      {details !== null && (
        <EventDetails
          event={details}
          plugins={plugins}
          onTune={(t) => {
            onTune(t);
            setDetails(null);
          }}
          onClose={() => setDetails(null)}
        />
      )}

      {confirmClear && (
        <ConfirmDialog
          title="Clear the event history?"
          confirmLabel="Delete all"
          confirmVariant="danger"
          busy={busy}
          onConfirm={() => void clear()}
          onCancel={() => setConfirmClear(false)}
        >
          All stored events (DTMF, tones, RDS and decoder plugins) are deleted permanently, for
          every receiver.
        </ConfirmDialog>
      )}
    </div>
  );
}

const timeCell = (e: DecoderEvent) => (
  <span
    title={formatFullTime(e.tsMs)}
    className="w-[3.4rem] shrink-0 font-mono text-[11px] text-fg-subtle num"
  >
    {formatTime(e.tsMs)}
  </span>
);

const vfoDot = (e: DecoderEvent) =>
  e.vfo === null ? (
    <span aria-hidden="true" className="size-2 shrink-0" />
  ) : (
    <span
      aria-label={`VFO ${e.vfo}`}
      title={`VFO ${e.vfo}`}
      className="size-2 shrink-0 rounded-full"
      style={{ background: vfoColor(e.vfo) }}
    />
  );

const freqCell = (e: DecoderEvent) => (
  <span className="w-[3.6rem] shrink-0 font-mono text-[11px] text-fg-muted num">
    {e.freqHz === null ? "–" : formatMhz(e.freqHz)}
  </span>
);

function Row({
  event: e,
  index,
  onTune,
  onDetails,
}: {
  event: DecoderEvent;
  index: number;
  onTune: (target: AnnotationTarget) => void;
  onDetails: (e: DecoderEvent) => void;
}) {
  const target = eventTarget(e);
  if (!isCoreEvent(e)) {
    return (
      <RichRow
        event={e}
        index={index}
        view={describeEvent(e)}
        onTune={target === null ? null : () => onTune(target)}
        onDetails={() => onDetails(e)}
      />
    );
  }
  const kind = e.kind as CoreKind;
  const note = eventNote(e);
  const mono = e.kind === "dtmf" || e.kind === "rds_pi";
  const cells = (
    <>
      {timeCell(e)}
      {vfoDot(e)}
      {freqCell(e)}
      <span className="shrink-0">
        <Badge tone={KIND_TONE[kind]} title={KIND_HINT[kind]}>
          {KIND_LABEL[kind]}
        </Badge>
      </span>
      <span
        title={e.value}
        className={cn(
          "min-w-0 flex-1 truncate text-xs text-fg",
          mono && "font-mono tracking-wider",
        )}
      >
        {e.value}
        {note !== null && <span className="ml-1.5 text-fg-subtle">{note}</span>}
      </span>
    </>
  );
  // Events without a frequency (a VFO that was tuned elsewhere) have nowhere to tune to.
  return (
    <div role="listitem" aria-posinset={index + 1} style={{ height: COMPACT_ROW_H }}>
      {target === null ? (
        <div className="flex h-full w-full items-center gap-2 px-3">{cells}</div>
      ) : (
        <PlainButton
          title={`Tune to ${formatMhz(target.freqHz)} MHz`}
          onClick={() => onTune(target)}
          className="flex h-full w-full items-center gap-2 px-3 text-left transition-colors duration-150 hover:bg-surface-2"
        >
          {cells}
        </PlainButton>
      )}
    </div>
  );
}

/** Two lines: who / what on top, the content below (pager text, position, readings). */
function RichRow({
  event: e,
  index,
  view,
  onTune,
  onDetails,
}: {
  event: DecoderEvent;
  index: number;
  view: EventView;
  onTune: (() => void) | null;
  onDetails: () => void;
}) {
  const body = (
    <>
      <span className="flex min-w-0 items-center gap-2">
        {timeCell(e)}
        <span className="shrink-0">
          <Badge tone={view.tone} title={view.title}>
            {view.label}
          </Badge>
        </span>
        <span
          className={cn("min-w-0 truncate text-xs font-medium text-fg", view.mono && "font-mono")}
        >
          {view.headline}
          {view.tags.length > 0 && (
            <span className="ml-1.5 font-sans font-normal text-fg-subtle">
              {view.tags.join(" · ")}
            </span>
          )}
        </span>
      </span>
      <span className="flex min-w-0 items-center gap-2">
        <span className="flex w-[3.4rem] shrink-0 items-center gap-1">
          {vfoDot(e)}
          <span className="font-mono text-[10px] text-fg-muted num">
            {e.freqHz === null ? "–" : formatMhz(e.freqHz)}
          </span>
        </span>
        <span className="min-w-0 truncate text-xs text-fg-muted">{view.detail ?? ""}</span>
      </span>
    </>
  );
  return (
    <div
      role="listitem"
      aria-posinset={index + 1}
      style={{ height: rowHeight(e) }}
      className="flex items-center gap-1 pr-1"
    >
      {onTune === null ? (
        <div className="flex min-w-0 flex-1 flex-col justify-center gap-1 px-3">{body}</div>
      ) : (
        <PlainButton
          title="Tune to where this was heard"
          onClick={onTune}
          className="flex h-full min-w-0 flex-1 flex-col justify-center gap-1 px-3 text-left transition-colors duration-150 hover:bg-surface-2"
        >
          {body}
        </PlainButton>
      )}
      {view.link !== null && (
        <a
          href={view.link.href}
          target="_blank"
          rel="noopener noreferrer"
          title="Open the position on OpenStreetMap"
          className="focus-ring shrink-0 rounded px-1.5 py-1 text-[11px] font-medium text-accent hover:bg-surface-2"
        >
          Map
        </a>
      )}
      <IconButton label="Event details" size="sm" onClick={onDetails}>
        <ChevronRightIcon />
      </IconButton>
    </div>
  );
}

function EventDetails({
  event: e,
  plugins,
  onTune,
  onClose,
}: {
  event: DecoderEvent;
  plugins: readonly PluginManifest[];
  onTune: (target: AnnotationTarget) => void;
  onClose: () => void;
}) {
  const view = describeEvent(e);
  const target = eventTarget(e);
  return (
    <Dialog title={view.headline} size="lg" onClose={onClose}>
      <div className="flex flex-col gap-3">
        <div className="flex flex-wrap items-center gap-2 text-xs text-fg-muted">
          <Badge tone={view.tone} title={view.title}>
            {view.label}
          </Badge>
          <span>{formatFullTime(e.tsMs)}</span>
          <span>{pluginName(plugins, e.source)}</span>
          {e.freqHz !== null && <span className="font-mono num">{formatMhz(e.freqHz)} MHz</span>}
          {e.vfo !== null && <span>VFO {e.vfo}</span>}
        </div>
        {view.detail !== null && (
          <p className="rounded-lg bg-surface-2 px-3 py-2 text-sm break-words whitespace-pre-wrap text-fg">
            {view.detail}
          </p>
        )}
        {view.fields.length > 0 && (
          <dl className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1 text-sm">
            {view.fields.map((f) => (
              <div key={f.label} className="contents">
                <dt className="text-fg-subtle">{f.label}</dt>
                <dd className={cn("min-w-0 break-words text-fg", f.mono && "font-mono")}>
                  {f.value}
                </dd>
              </div>
            ))}
          </dl>
        )}
        {view.raw !== null && (
          <details className="text-xs text-fg-muted">
            <summary className="cursor-pointer select-none">Raw data</summary>
            <pre className="mt-2 max-h-56 overflow-auto rounded-md bg-surface-2 p-2 font-mono text-[11px] text-fg">
              {view.raw}
            </pre>
          </details>
        )}
        <div className="flex flex-wrap justify-end gap-2">
          {view.link !== null && (
            <a
              href={view.link.href}
              target="_blank"
              rel="noopener noreferrer"
              className="focus-ring inline-flex h-8 items-center rounded-md border border-border-strong bg-surface-1 px-3 text-sm font-medium text-fg hover:bg-surface-2"
            >
              {view.link.label}
            </a>
          )}
          {target !== null && (
            <Button onClick={() => onTune(target)}>Tune to {formatMhz(target.freqHz)} MHz</Button>
          )}
          <Button variant="secondary" onClick={onClose}>
            Close
          </Button>
        </div>
      </div>
    </Dialog>
  );
}
