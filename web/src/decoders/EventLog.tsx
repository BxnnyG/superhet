import { useEffect, useLayoutEffect, useMemo, useRef, useState, useSyncExternalStore } from "react";
import { usePermissions } from "../auth/context";
import { ActionsMenu } from "../bookmarks/ActionsMenu";
import { layoutRows, visibleWindow } from "../bookmarks/virtual";
import type { AnnotationTarget } from "../spectrum/annotations";
import {
  Alert,
  Badge,
  Button,
  Chip,
  ConfirmDialog,
  EmptyState,
  MoreIcon,
  PauseIcon,
  PlainButton,
  SearchIcon,
  cn,
  toasts,
  type BadgeTone,
} from "../ui";
import { vfoColor } from "../vfo/model";
import { downloadEventLog } from "./api";
import {
  KIND_CHIPS,
  KIND_HINT,
  KIND_LABEL,
  eventNote,
  eventTarget,
  formatFullTime,
  formatMhz,
  formatTime,
  type DecoderEvent,
  type EventKind,
} from "./events";
import type { DecoderEventStore } from "./store";

const ROW_H = 32;
/** Viewport height assumed until the list has been measured (and in jsdom). */
const FALLBACK_VIEWPORT_H = 360;
const SEARCH_DEBOUNCE_MS = 250;

const KIND_TONE: Record<EventKind, BadgeTone> = {
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
  onTune: (target: AnnotationTarget) => void;
}

/**
 * The decoder event log: filter chips, search, pause, export and clear in a pinned header, then a
 * windowed table (fixed row height, newest first). Fills its container; only the table scrolls.
 * A click on a row tunes to the frequency the event was heard on.
 */
export function EventLog({ store, device, onTune }: Props) {
  const snap = useSyncExternalStore(store.subscribe, store.getSnapshot);
  const { events, filter } = snap;
  const [query, setQuery] = useState(filter.q);
  const [confirmClear, setConfirmClear] = useState(false);
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
  const toggleChip = (kinds: readonly EventKind[]) => {
    const on = kinds.every((k) => activeKinds.includes(k));
    const next = on
      ? activeKinds.filter((k) => !kinds.includes(k))
      : [...activeKinds, ...kinds.filter((k) => !activeKinds.includes(k))];
    store.setFilter({ ...filter, kinds: next });
  };

  // Windowing.
  const heights = useMemo(() => events.map(() => ROW_H), [events]);
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
    if (at > 0) el.scrollTop += at * ROW_H;
  }, [events]);

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

  const filtering = filter.kinds.length > 0 || filter.q.trim() !== "";
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
        <div className="flex flex-wrap gap-1" role="group" aria-label="Event kinds">
          {KIND_CHIPS.map((chip) => (
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
                : "DTMF digits, CTCSS tones and RDS changes are logged here as they are decoded."}
            </EmptyState>
          </div>
        )}
        {events.length > 0 && (
          <div role="list" aria-label="Decoder events">
            <div style={{ height: layout.offsets[win.start] ?? 0 }} aria-hidden="true" />
            {events.slice(win.start, win.end).map((e, i) => (
              <Row key={e.id} event={e} index={win.start + i} onTune={onTune} />
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

      {confirmClear && (
        <ConfirmDialog
          title="Clear the event history?"
          confirmLabel="Delete all"
          confirmVariant="danger"
          busy={busy}
          onConfirm={() => void clear()}
          onCancel={() => setConfirmClear(false)}
        >
          All stored DTMF, tone and RDS events are deleted permanently, for every receiver.
        </ConfirmDialog>
      )}
    </div>
  );
}

function Row({
  event: e,
  index,
  onTune,
}: {
  event: DecoderEvent;
  index: number;
  onTune: (target: AnnotationTarget) => void;
}) {
  const target = eventTarget(e);
  const note = eventNote(e);
  const mono = e.kind === "dtmf" || e.kind === "rds_pi";
  const cells = (
    <>
      <span
        title={formatFullTime(e.tsMs)}
        className="w-[3.4rem] shrink-0 font-mono text-[11px] text-fg-subtle num"
      >
        {formatTime(e.tsMs)}
      </span>
      <span
        aria-label={`VFO ${e.vfo}`}
        title={`VFO ${e.vfo}`}
        className="size-2 shrink-0 rounded-full"
        style={{ background: vfoColor(e.vfo) }}
      />
      <span className="w-[3.6rem] shrink-0 font-mono text-[11px] text-fg-muted num">
        {e.freqHz === null ? "–" : formatMhz(e.freqHz)}
      </span>
      <span className="shrink-0">
        <Badge tone={KIND_TONE[e.kind]} title={KIND_HINT[e.kind]}>
          {KIND_LABEL[e.kind]}
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
    <div role="listitem" aria-posinset={index + 1} style={{ height: ROW_H }}>
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
