import { useMemo, useRef, useState, useSyncExternalStore, type ChangeEvent } from "react";
import type { OverlayStore } from "../bandplan/settings";
import type { AnnotationTarget } from "../spectrum/annotations";
import { formatFrequency } from "../spectrum/freq";
import {
  Alert,
  BookmarkIcon,
  Button,
  ConfirmDialog,
  Dialog,
  EmptyState,
  IconButton,
  PencilIcon,
  RowButton,
  SegmentedControl,
  Select,
  TextField,
  Toggle,
  TrashIcon,
  toasts,
} from "../ui";
import { MODE_LABEL } from "../vfo/model";
import {
  FORMAT_LABEL,
  exportGqrx,
  exportSuperhet,
  parseImport,
  type ImportResult,
} from "./formats";
import { allTags, bookmarkColor, filterBookmarks, newBookmarks, type Bookmark } from "./model";
import type { BookmarkStore } from "./store";

interface Props {
  store: BookmarkStore;
  overlay: OverlayStore;
  /** Frequency of the active VFO, to highlight its bookmark. */
  activeFreqHz: number | null;
  onTune: (target: AnnotationTarget) => void;
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

/** Bookmarks section: list with search and tag filter, add/edit/delete, import/export. */
export function BookmarksPanel({ store, overlay, activeFreqHz, onTune, onAdd }: Props) {
  const snap = useSyncExternalStore(store.subscribe, store.getSnapshot);
  const show = useSyncExternalStore(overlay.subscribe, overlay.getSnapshot).bookmarks;
  const [query, setQuery] = useState("");
  const [tag, setTag] = useState("");
  const [deleting, setDeleting] = useState<Bookmark | null>(null);
  const [busy, setBusy] = useState(false);
  const [importing, setImporting] = useState<ImportResult | null>(null);
  const [importError, setImportError] = useState<string | null>(null);
  const fileRef = useRef<HTMLInputElement>(null);

  const tags = useMemo(() => allTags(snap.bookmarks), [snap.bookmarks]);
  const shown = useMemo(
    () => filterBookmarks(snap.bookmarks, query, tags.includes(tag) ? tag : ""),
    [snap.bookmarks, query, tag, tags],
  );

  const remove = async (b: Bookmark) => {
    setBusy(true);
    const error = await store.remove(b.id);
    setBusy(false);
    setDeleting(null);
    if (error !== null) {
      toasts.push({ tone: "danger", title: "Could not delete the bookmark", body: error });
    }
  };

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

  return (
    <div className="flex flex-col gap-3" role="group" aria-label="Bookmarks">
      <Toggle
        label="Show on spectrum"
        checked={show}
        onChange={(on) => overlay.set({ bookmarks: on })}
      />
      <Button icon={<BookmarkIcon />} onClick={onAdd}>
        Add bookmark…
      </Button>
      {snap.error !== null && (
        <Alert tone="warn">
          <p>Bookmarks are unavailable: {snap.error}</p>
          <Button size="sm" className="mt-2" onClick={() => void store.refresh()}>
            Retry
          </Button>
        </Alert>
      )}
      {snap.bookmarks.length > 0 && (
        <div className="flex flex-col gap-2">
          <TextField
            label="Search bookmarks"
            type="search"
            value={query}
            autoComplete="off"
            onChange={(e) => setQuery(e.target.value)}
          />
          {tags.length > 0 && (
            <Select
              label="Tag"
              value={tags.includes(tag) ? tag : ""}
              onChange={setTag}
              options={[
                { value: "", label: "All tags" },
                ...tags.map((t) => ({ value: t, label: t })),
              ]}
            />
          )}
        </div>
      )}
      {!snap.loaded && snap.error === null && <p className="text-xs text-fg-subtle">Loading…</p>}
      {snap.loaded && snap.error === null && snap.bookmarks.length === 0 && (
        <EmptyState title="No bookmarks yet">
          Press the star next to the frequency or “Add bookmark…”, or import a list.
        </EmptyState>
      )}
      {snap.bookmarks.length > 0 && shown.length === 0 && (
        <p className="text-xs text-fg-muted">No bookmark matches.</p>
      )}
      {shown.length > 0 && (
        <ul className="flex flex-col gap-0.5" aria-label="Bookmark list">
          {shown.map((b) => (
            <li key={b.id} className="flex items-center gap-0.5">
              <RowButton
                selected={
                  activeFreqHz !== null && Math.round(activeFreqHz) === Math.round(b.freqHz)
                }
                className="flex-1"
                aria-label={`Tune to ${b.name}`}
                title={b.tags.length > 0 ? b.tags.join(", ") : undefined}
                onClick={() =>
                  onTune({
                    freqHz: b.freqHz,
                    ...(b.mode ? { mode: b.mode } : {}),
                    ...(b.bandwidthHz ? { bandwidthHz: b.bandwidthHz } : {}),
                  })
                }
              >
                <span
                  aria-hidden="true"
                  className="size-2 shrink-0 rounded-full"
                  style={{ background: bookmarkColor(b) }}
                />
                <span className="min-w-0 flex-1">
                  <span className="block truncate text-sm leading-tight text-fg">{b.name}</span>
                  <span className="block truncate text-xs leading-tight text-fg-muted num">
                    {formatFrequency(b.freqHz).slice(0, -4)} MHz
                    {b.mode ? ` · ${MODE_LABEL[b.mode]}` : ""}
                  </span>
                </span>
              </RowButton>
              <IconButton
                label={`Edit bookmark ${b.name}`}
                size="sm"
                onClick={() => store.openEditor({ kind: "edit", bookmark: b })}
              >
                <PencilIcon size={14} />
              </IconButton>
              <IconButton
                label={`Delete bookmark ${b.name}`}
                size="sm"
                disabled={busy}
                onClick={() => setDeleting(b)}
              >
                <TrashIcon size={14} />
              </IconButton>
            </li>
          ))}
        </ul>
      )}

      <div className="flex flex-wrap gap-2" role="group" aria-label="Import and export">
        <Button size="sm" onClick={() => fileRef.current?.click()}>
          Import…
        </Button>
        <Button
          size="sm"
          disabled={snap.bookmarks.length === 0}
          onClick={() =>
            download("superhet-bookmarks.json", "application/json", exportSuperhet(snap.bookmarks))
          }
        >
          Export JSON
        </Button>
        <Button
          size="sm"
          disabled={snap.bookmarks.length === 0}
          onClick={() => download("bookmarks.csv", "text/csv", exportGqrx(snap.bookmarks))}
        >
          Export gqrx CSV
        </Button>
        <input
          ref={fileRef}
          type="file"
          accept=".json,.csv,.txt,application/json,text/csv,text/plain"
          aria-label="Bookmark file to import"
          className="sr-only"
          tabIndex={-1}
          onChange={(e) => void onFile(e)}
        />
      </div>
      {importError !== null && <Alert tone="danger">{importError}</Alert>}

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
