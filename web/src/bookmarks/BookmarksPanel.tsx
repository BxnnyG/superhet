import {
  memo,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
  type ChangeEvent,
  type MouseEvent,
} from "react";
import type { OverlayStore } from "../bandplan/settings";
import type { AnnotationTarget } from "../spectrum/annotations";
import { formatFrequency } from "../spectrum/freq";
import {
  Alert,
  Badge,
  Button,
  ChevronDownIcon,
  ChevronRightIcon,
  ConfirmDialog,
  Dialog,
  EmptyState,
  IconButton,
  MoreIcon,
  PlainButton,
  PencilIcon,
  PlusIcon,
  SegmentedControl,
  Select,
  StarIcon,
  TrashIcon,
  cn,
  toasts,
} from "../ui";
import { MODE_LABEL } from "../vfo/model";
import { ActionsMenu } from "./ActionsMenu";
import {
  FORMAT_LABEL,
  exportChirp,
  exportGqrx,
  exportSuperhet,
  parseImport,
  type ImportResult,
} from "./formats";
import { GroupsDialog } from "./GroupsDialog";
import {
  NO_FILTER,
  SORT_LABEL,
  allTags,
  bookmarkColor,
  bookmarkTarget,
  digitalBadge,
  filterBookmarks,
  groupSections,
  isFiltering,
  nearBookmarks,
  newBookmarks,
  sortBy,
  toneBadge,
  type Bookmark,
  type Filter,
  type SortKey,
} from "./model";
import { loadPrefs, savePrefs } from "./prefs";
import type { BookmarkStore } from "./store";
import { layoutRows, visibleWindow } from "./virtual";

interface Props {
  store: BookmarkStore;
  overlay: OverlayStore;
  /** Frequency of the active VFO, to highlight its bookmark. */
  activeFreqHz: number | null;
  /** Tune the selected VFO and apply the bookmark's stored receiver state. */
  onTune: (target: AnnotationTarget) => void;
  /** Alt-click: open the bookmark on a new VFO. Alt-click tunes like a plain click without it. */
  onTuneNew?: (target: AnnotationTarget) => void;
  /** Open the add dialog prefilled from the receiver. */
  onAdd: () => void;
}

function readText(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(typeof reader.result === "string" ? reader.result : "");
    reader.onerror = () => reject(new Error("Could not read the file."));
    reader.readAsText(file);
  });
}

function download(name: string, mime: string, text: string): void {
  const url = URL.createObjectURL(new Blob([text], { type: mime }));
  const a = document.createElement("a");
  a.href = url;
  a.download = name;
  document.body.append(a);
  a.click();
  a.remove();
  setTimeout(() => URL.revokeObjectURL(url), 0);
}

const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;

const HEAD_H = 28;
const ITEM_H = 44;
/** Viewport height assumed until the list has been measured (and in jsdom). */
const FALLBACK_VIEWPORT_H = 480;
const UNGROUPED = "none";

type Row =
  | { kind: "near"; key: string }
  | { kind: "group"; key: string; id: string; title: string; count: number; folded: boolean }
  | { kind: "item"; key: string; bookmark: Bookmark };

const rowHeight = (r: Row) => (r.kind === "item" ? ITEM_H : HEAD_H);

/**
 * Bookmarks section: pinned header (search, group and tag filter, sort, add, "…" menu), then the
 * list grouped by folder with collapsible groups, a "Near current frequency" block and a
 * windowed row list so thousands of bookmarks stay cheap. Fills its container; only the list
 * scrolls.
 */
export function BookmarksPanel({ store, overlay, activeFreqHz, onTune, onTuneNew, onAdd }: Props) {
  const snap = useSyncExternalStore(store.subscribe, store.getSnapshot);
  const show = useSyncExternalStore(overlay.subscribe, overlay.getSnapshot).bookmarks;
  const [filter, setFilter] = useState<Filter>(NO_FILTER);
  const [prefs, setPrefs] = useState(() => loadPrefs());
  const [deleting, setDeleting] = useState<Bookmark | null>(null);
  const [busy, setBusy] = useState(false);
  const [importing, setImporting] = useState<ImportResult | null>(null);
  const [importError, setImportError] = useState<string | null>(null);
  const [managingGroups, setManagingGroups] = useState(false);
  const fileRef = useRef<HTMLInputElement>(null);

  const updatePrefs = (next: typeof prefs) => {
    setPrefs(next);
    savePrefs(next);
  };

  const tags = useMemo(() => allTags(snap.bookmarks), [snap.bookmarks]);
  const tag = tags.includes(filter.tag) ? filter.tag : "";
  const groupFilter =
    filter.group === "all" ||
    filter.group === UNGROUPED ||
    snap.groups.some((g) => g.id === filter.group)
      ? filter.group
      : "all";
  const effective: Filter = { ...filter, tag, group: groupFilter };
  const filtering = isFiltering(effective);

  const shown = useMemo(
    () =>
      sortBy(
        filterBookmarks(snap.bookmarks, effective.query, tag, {
          group: groupFilter,
          favouritesOnly: effective.favouritesOnly,
        }),
        prefs.sort,
      ),
    [snap.bookmarks, effective.query, tag, groupFilter, effective.favouritesOnly, prefs.sort],
  );
  const near = useMemo(
    () => (filtering ? [] : nearBookmarks(snap.bookmarks, activeFreqHz)),
    [filtering, snap.bookmarks, activeFreqHz],
  );

  const rows = useMemo(() => {
    const out: Row[] = [];
    if (near.length > 0) {
      out.push({ kind: "near", key: "near" });
      for (const b of near) out.push({ kind: "item", key: `near:${b.id}`, bookmark: b });
    }
    const sections = groupSections(shown, snap.groups, filtering);
    const flat = sections.length === 1 && sections[0]?.group === null;
    for (const s of sections) {
      const id = s.group?.id ?? UNGROUPED;
      const folded = !flat && !filtering && prefs.collapsed.includes(id);
      if (!flat) {
        out.push({
          kind: "group",
          key: `group:${id}`,
          id,
          title: s.group?.name ?? "Ungrouped",
          count: s.items.length,
          folded,
        });
      }
      if (!folded) for (const b of s.items) out.push({ kind: "item", key: b.id, bookmark: b });
    }
    return out;
  }, [near, shown, snap.groups, filtering, prefs.collapsed]);

  const heights = useMemo(() => rows.map(rowHeight), [rows]);
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

  const toggleFold = (id: string) =>
    updatePrefs({
      ...prefs,
      collapsed: prefs.collapsed.includes(id)
        ? prefs.collapsed.filter((x) => x !== id)
        : [...prefs.collapsed, id],
    });

  const remove = async (b: Bookmark) => {
    setBusy(true);
    const error = await store.remove(b.id);
    setBusy(false);
    setDeleting(null);
    if (error !== null) {
      toasts.push({ tone: "danger", title: "Could not delete the bookmark", body: error });
    }
  };

  const toggleFavourite = useCallback(
    async (b: Bookmark) => {
      const error = await store.update(b.id, { favourite: !b.favourite });
      if (error !== null) {
        toasts.push({ tone: "danger", title: "Could not update the bookmark", body: error });
      }
    },
    [store],
  );

  const tune = useCallback(
    (b: Bookmark, e: MouseEvent) => {
      const target = bookmarkTarget(b);
      if (e.altKey && onTuneNew) onTuneNew(target);
      else onTune(target);
    },
    [onTune, onTuneNew],
  );
  const edit = useCallback(
    (b: Bookmark) => store.openEditor({ kind: "edit", bookmark: b }),
    [store],
  );

  const onFile = async (e: ChangeEvent<HTMLInputElement>) => {
    const file = e.target.files?.[0];
    e.target.value = "";
    if (!file) return;
    try {
      setImporting(parseImport(await readText(file)));
      setImportError(null);
    } catch (err) {
      setImporting(null);
      setImportError(err instanceof Error ? err.message : String(err));
    }
  };

  const empty = snap.bookmarks.length === 0;
  const set = (patch: Partial<Filter>) => setFilter({ ...effective, ...patch });

  return (
    <div className="flex h-full min-h-0 flex-col" role="group" aria-label="Bookmarks">
      <div className="flex shrink-0 flex-col gap-2 border-b border-border pb-2">
        <div className="flex items-center gap-1">
          <input
            type="search"
            aria-label="Search bookmarks"
            placeholder="Search name, tag, MHz, tone…"
            value={filter.query}
            autoComplete="off"
            onChange={(e) => set({ query: e.target.value })}
            className="focus-ring h-8 min-w-0 flex-1 rounded-md border border-border-strong bg-surface-1 px-2.5 text-sm text-fg placeholder:text-fg-subtle"
          />
          <IconButton label="Add bookmark…" onClick={onAdd}>
            <PlusIcon />
          </IconButton>
          <ActionsMenu
            label="More bookmark actions"
            icon={<MoreIcon />}
            actions={[
              { label: "Import…", onSelect: () => fileRef.current?.click() },
              {
                label: "Export JSON",
                disabled: empty,
                onSelect: () =>
                  download(
                    "superhet-bookmarks.json",
                    "application/json",
                    exportSuperhet(snap.bookmarks, snap.groups),
                  ),
              },
              {
                label: "Export gqrx CSV",
                disabled: empty,
                onSelect: () => download("bookmarks.csv", "text/csv", exportGqrx(snap.bookmarks)),
              },
              {
                label: "Export CHIRP CSV",
                disabled: empty,
                onSelect: () =>
                  download("bookmarks-chirp.csv", "text/csv", exportChirp(snap.bookmarks)),
              },
              { label: "Manage groups…", separated: true, onSelect: () => setManagingGroups(true) },
              {
                label: "Show on spectrum",
                checked: show,
                onSelect: () => overlay.set({ bookmarks: !show }),
              },
            ]}
          />
        </div>
        {!empty && (
          <div className="flex items-center gap-1">
            <Select
              aria-label="Group"
              size="sm"
              className="flex-1"
              value={groupFilter}
              onChange={(group) => set({ group })}
              options={[
                { value: "all", label: "All groups" },
                ...snap.groups.map((g) => ({ value: g.id, label: g.name })),
                { value: UNGROUPED, label: "Ungrouped" },
              ]}
            />
            {tags.length > 0 && (
              <Select
                aria-label="Tag"
                size="sm"
                className="flex-1"
                value={tag}
                onChange={(t) => set({ tag: t })}
                options={[
                  { value: "", label: "All tags" },
                  ...tags.map((t) => ({ value: t, label: t })),
                ]}
              />
            )}
            <Select
              aria-label="Sort bookmarks"
              size="sm"
              className="flex-1"
              value={prefs.sort}
              onChange={(sort) => updatePrefs({ ...prefs, sort: sort as SortKey })}
              options={(Object.keys(SORT_LABEL) as SortKey[]).map((k) => ({
                value: k,
                label: SORT_LABEL[k],
              }))}
            />
            <IconButton
              label="Favourites only"
              size="sm"
              pressed={filter.favouritesOnly}
              onClick={() => set({ favouritesOnly: !filter.favouritesOnly })}
            >
              <StarIcon size={14} fill={filter.favouritesOnly ? "currentColor" : "none"} />
            </IconButton>
          </div>
        )}
      </div>

      <div
        ref={scroller}
        className="min-h-0 flex-1 overflow-y-auto pt-2"
        onScroll={(e) => setScrollTop(e.currentTarget.scrollTop)}
      >
        {snap.error !== null && (
          <Alert tone="warn">
            <p>Bookmarks are unavailable: {snap.error}</p>
            <Button size="sm" className="mt-2" onClick={() => void store.refresh()}>
              Retry
            </Button>
          </Alert>
        )}
        {!snap.loaded && snap.error === null && <p className="text-xs text-fg-subtle">Loading…</p>}
        {snap.loaded && snap.error === null && empty && (
          <EmptyState title="No bookmarks yet">
            Press the star next to the frequency or the + above, or import a list from the “…” menu.
          </EmptyState>
        )}
        {!empty && shown.length === 0 && (
          <p className="text-xs text-fg-muted">No bookmark matches.</p>
        )}
        {rows.length > 0 && (
          <div role="list" aria-label="Bookmark list">
            <div style={{ height: layout.offsets[win.start] ?? 0 }} aria-hidden="true" />
            {rows.slice(win.start, win.end).map((row) => {
              if (row.kind === "near") {
                return (
                  <div
                    key={row.key}
                    role="presentation"
                    style={{ height: HEAD_H }}
                    className="flex items-center px-1 text-[11px] font-medium tracking-wide text-fg-subtle uppercase"
                  >
                    Near current frequency
                  </div>
                );
              }
              if (row.kind === "group") {
                return (
                  <PlainButton
                    key={row.key}
                    aria-expanded={!row.folded}
                    disabled={filtering}
                    style={{ height: HEAD_H }}
                    onClick={() => toggleFold(row.id)}
                    className="flex w-full items-center gap-1.5 rounded-md px-1 text-left text-[11px] font-medium tracking-wide text-fg-subtle uppercase hover:text-fg"
                  >
                    {row.folded ? <ChevronRightIcon size={12} /> : <ChevronDownIcon size={12} />}
                    <span className="min-w-0 flex-1 truncate">{row.title}</span>
                    <span className="num">{row.count}</span>
                  </PlainButton>
                );
              }
              return (
                <BookmarkRow
                  key={row.key}
                  bookmark={row.bookmark}
                  active={
                    activeFreqHz !== null &&
                    Math.round(activeFreqHz) === Math.round(row.bookmark.freqHz)
                  }
                  busy={busy}
                  onTune={tune}
                  onEdit={edit}
                  onDelete={setDeleting}
                  onFavourite={toggleFavourite}
                />
              );
            })}
            <div
              style={{
                height: Math.max(0, layout.total - (layout.offsets[win.end] ?? layout.total)),
              }}
              aria-hidden="true"
            />
          </div>
        )}
        {importError !== null && (
          <div className="pt-2">
            <Alert tone="danger">{importError}</Alert>
          </div>
        )}
      </div>

      <input
        ref={fileRef}
        type="file"
        accept=".json,.csv,.txt,application/json,text/csv,text/plain"
        aria-label="Bookmark file to import"
        className="sr-only"
        tabIndex={-1}
        onChange={(e) => void onFile(e)}
      />

      {deleting !== null && (
        <ConfirmDialog
          title="Delete bookmark?"
          confirmLabel="Delete"
          confirmVariant="danger"
          busy={busy}
          onCancel={() => setDeleting(null)}
          onConfirm={() => void remove(deleting)}
        >
          “{deleting.name}” will be removed permanently.
        </ConfirmDialog>
      )}
      {managingGroups && <GroupsDialog store={store} onClose={() => setManagingGroups(false)} />}
      {importing !== null && (
        <ImportDialog
          result={importing}
          existing={snap.bookmarks}
          onCancel={() => setImporting(null)}
          onImport={async (items, replace) => {
            const r = await store.bulk(items, replace);
            if ("error" in r) return r.error;
            toasts.push({
              tone: "success",
              title: `Imported ${plural(r.created, "bookmark")}`,
              key: "bookmark",
            });
            setImporting(null);
            return null;
          }}
        />
      )}
    </div>
  );
}

interface RowProps {
  bookmark: Bookmark;
  active: boolean;
  busy: boolean;
  onTune: (b: Bookmark, e: MouseEvent) => void;
  onEdit: (b: Bookmark) => void;
  onDelete: (b: Bookmark) => void;
  onFavourite: (b: Bookmark) => void;
}

/** "name · freq · mode · tone badge" with edit and delete on hover (always shown on touch). */
const BookmarkRow = memo(function BookmarkRow({
  bookmark: b,
  active,
  busy,
  onTune,
  onEdit,
  onDelete,
  onFavourite,
}: RowProps) {
  const tone = toneBadge(b);
  const digital = digitalBadge(b);
  return (
    <div role="listitem" style={{ height: ITEM_H }} className="group flex items-center gap-0.5">
      <PlainButton
        aria-pressed={active}
        aria-label={`Tune to ${b.name}`}
        title={[
          b.tags.length > 0 ? b.tags.join(", ") : null,
          "Click to tune, Alt-click for a new VFO, double-click to edit",
        ]
          .filter(Boolean)
          .join(" · ")}
        onClick={(e) => onTune(b, e)}
        onDoubleClick={() => onEdit(b)}
        className={cn(
          "flex h-10 min-w-0 flex-1 items-center gap-2 rounded-md border px-2.5 text-left",
          "transition-colors duration-150",
          active ? "border-accent/50 bg-accent-soft" : "border-transparent hover:bg-surface-2",
        )}
      >
        <span
          aria-hidden="true"
          className="size-2 shrink-0 rounded-full"
          style={{ background: bookmarkColor(b) }}
        />
        <span className="min-w-0 flex-1">
          <span className="flex items-center gap-1 text-sm leading-tight text-fg">
            <span className="truncate">{b.name}</span>
            {b.favourite && (
              <StarIcon size={11} fill="currentColor" className="shrink-0 text-warn" />
            )}
          </span>
          <span className="flex items-center gap-1.5 text-xs leading-tight text-fg-muted">
            <span className="truncate num">
              {formatFrequency(b.freqHz).slice(0, -4)} MHz
              {b.mode ? ` · ${MODE_LABEL[b.mode]}` : ""}
            </span>
            {tone !== null && (
              <Badge
                tone="accent"
                title={b.toneMode === "dcs" ? "DCS (stored only, not detected yet)" : "CTCSS"}
                className="h-4 shrink-0 px-1.5 text-[10px]"
              >
                {tone}
              </Badge>
            )}
            {digital !== null && (
              <Badge tone="neutral" className="h-4 shrink-0 px-1.5 text-[10px]">
                {digital}
              </Badge>
            )}
          </span>
        </span>
      </PlainButton>
      <div
        className={cn(
          "flex shrink-0 items-center opacity-0 transition-opacity",
          "group-hover:opacity-100 focus-within:opacity-100 [@media(hover:none)]:opacity-100",
        )}
      >
        <IconButton
          label={b.favourite ? `Unfavourite ${b.name}` : `Favourite ${b.name}`}
          size="sm"
          onClick={() => onFavourite(b)}
        >
          <StarIcon size={14} fill={b.favourite ? "currentColor" : "none"} />
        </IconButton>
        <IconButton label={`Edit bookmark ${b.name}`} size="sm" onClick={() => onEdit(b)}>
          <PencilIcon size={14} />
        </IconButton>
        <IconButton
          label={`Delete bookmark ${b.name}`}
          size="sm"
          disabled={busy}
          onClick={() => onDelete(b)}
        >
          <TrashIcon size={14} />
        </IconButton>
      </div>
    </div>
  );
});

type ImportMode = "merge" | "replace";

interface ImportProps {
  result: ImportResult;
  existing: readonly Bookmark[];
  onCancel: () => void;
  /** Resolves with an error text, or null when done. */
  onImport: (items: ImportResult["bookmarks"], replace: boolean) => Promise<string | null>;
}

/** Preview of an import file: counts, and whether to merge with or replace the current list. */
function ImportDialog({ result, existing, onCancel, onImport }: ImportProps) {
  const [mode, setMode] = useState<ImportMode>("merge");
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const fresh = useMemo(() => newBookmarks(existing, result.bookmarks), [existing, result]);
  const items = mode === "merge" ? fresh : result.bookmarks;

  const run = async () => {
    setPending(true);
    setError(null);
    const e = await onImport(items, mode === "replace");
    setPending(false);
    if (e !== null) setError(e);
  };

  return (
    <Dialog title="Import bookmarks" onClose={onCancel}>
      <div className="flex flex-col gap-3 text-sm">
        <p>
          <span className="font-medium">{FORMAT_LABEL[result.format]}</span>:{" "}
          {plural(result.bookmarks.length, "bookmark")} found.
        </p>
        <ul className="list-disc space-y-0.5 pl-5 text-xs text-fg-muted">
          <li>
            {fresh.length} new, {result.bookmarks.length - fresh.length} already in your list.
          </li>
          {result.skipped > 0 && (
            <li>{plural(result.skipped, "entry")} skipped (no valid frequency).</li>
          )}
          {result.unsupportedMode > 0 && (
            <li>
              {plural(result.unsupportedMode, "entry")} use a modulation Superhet has no mode for;
              imported without a mode, the original is kept in the notes.
            </li>
          )}
        </ul>
        <SegmentedControl<ImportMode>
          aria-label="Import mode"
          value={mode}
          onChange={setMode}
          options={[
            { value: "merge", label: "Merge" },
            { value: "replace", label: "Replace all" },
          ]}
        />
        <p className="text-xs text-fg-muted">
          {mode === "merge"
            ? `Adds ${plural(fresh.length, "bookmark")} and keeps the ${existing.length} you have.`
            : `Deletes your ${plural(existing.length, "current bookmark")} and imports ${result.bookmarks.length}.`}
        </p>
        {error !== null && <Alert tone="danger">{error}</Alert>}
        <div className="flex justify-end gap-2">
          <Button onClick={onCancel}>Cancel</Button>
          <Button
            variant={mode === "replace" ? "danger" : "primary"}
            disabled={pending || (items.length === 0 && mode === "merge")}
            onClick={() => void run()}
          >
            {mode === "replace" ? "Replace and import" : `Import ${items.length}`}
          </Button>
        </div>
      </div>
    </Dialog>
  );
}
