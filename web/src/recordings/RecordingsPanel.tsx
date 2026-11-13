import { useState } from "react";
import { usePermissions } from "../auth/context";
import { formatDateTime } from "../format";
import {
  Badge,
  Button,
  ConfirmDialog,
  DownloadIcon,
  EmptyState,
  IconButton,
  PencilIcon,
  PlayIcon,
  Select,
  StarIcon,
  StopIcon,
  TextField,
  TrashIcon,
  toasts,
} from "../ui";
import { type Recording, type RecordingsApi, fileUrl, recordingsApi } from "./api";
import { STATE_TEXT, endReason, formatBytes, formatClock, isRunning, subtitle } from "./model";
import { useRecordings } from "./useRecordings";

const KIND_OPTIONS = [
  { value: "", label: "All" },
  { value: "audio", label: "Audio" },
  { value: "iq", label: "IQ" },
];

const fail = (title: string, e: unknown) =>
  toasts.push({ tone: "danger", title, body: e instanceof Error ? e.message : String(e) });

/**
 * The Recordings tab: audio and IQ recordings of this device (and saved replays), playable with
 * seeking, downloadable, renamable, protectable from the quota cleanup (Keep) and deletable.
 * Everybody with an account lists and plays; operators and admins manage.
 */
export function RecordingsPanel({
  device,
  api = recordingsApi,
}: {
  device: string;
  api?: RecordingsApi;
}) {
  const perms = usePermissions();
  const canManage = perms.isOperator && !perms.deviceReadOnly;
  const [kind, setKind] = useState("");
  const [search, setSearch] = useState("");
  const list = useRecordings(
    {
      dev: device,
      kind: kind === "" ? undefined : (kind as "audio" | "iq"),
      q: search.trim() === "" ? undefined : search.trim(),
    },
    { api },
  );
  const [playing, setPlaying] = useState<number | null>(null);
  const [renaming, setRenaming] = useState<number | null>(null);
  const [draft, setDraft] = useState("");
  const [toDelete, setToDelete] = useState<Recording | null>(null);
  const [busy, setBusy] = useState(false);

  const act = async (title: string, run: () => Promise<unknown>) => {
    setBusy(true);
    try {
      await run();
      list.reload();
    } catch (e) {
      fail(title, e);
    } finally {
      setBusy(false);
    }
  };

  const saveName = (r: Recording) => {
    const title = draft.trim();
    setRenaming(null);
    if (title === "" || title === r.title) return;
    void act("Cannot rename", () => api.update(r.id, { title }));
  };

  return (
    <div className="flex h-full min-h-0 flex-col" aria-label="Recordings">
      <div className="grid grid-cols-2 gap-2 border-b border-border px-4 py-3">
        <TextField label="Search" value={search} onChange={(e) => setSearch(e.target.value)} />
        <Select label="Type" value={kind} options={KIND_OPTIONS} onChange={setKind} />
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto">
        {list.error !== null && (
          <p role="alert" className="px-4 py-3 text-sm text-danger">
            {list.error}
          </p>
        )}
        {list.items.length === 0 && list.error === null && !list.loading && (
          <div className="p-4">
            <EmptyState title="No recordings yet">
              {canManage
                ? "Press Record on a receiver, or save an instant replay."
                : "Operators record from the Receiver tab."}
            </EmptyState>
          </div>
        )}
        <ul className="flex flex-col">
          {list.items.map((r) => (
            <li
              key={r.id}
              className="flex flex-col gap-1.5 border-b border-border px-4 py-3"
              data-testid="recording-row"
            >
              <div className="flex items-start gap-2">
                <div className="min-w-0 flex-1">
                  {renaming === r.id ? (
                    <TextField
                      label="Title"
                      value={draft}
                      autoFocus
                      maxLength={200}
                      onChange={(e) => setDraft(e.target.value)}
                      onBlur={() => saveName(r)}
                      onKeyDown={(e) => {
                        if (e.key === "Enter") saveName(r);
                        if (e.key === "Escape") setRenaming(null);
                      }}
                    />
                  ) : (
                    <p className="truncate text-sm font-medium" title={r.title}>
                      {r.title || `Recording ${r.id}`}
                    </p>
                  )}
                  <p className="truncate text-xs text-fg-muted">{subtitle(r)}</p>
                </div>
                {isRunning(r) ? (
                  <Badge tone="danger">Recording</Badge>
                ) : r.state !== "done" ? (
                  <Badge tone="warn">{STATE_TEXT[r.state]}</Badge>
                ) : null}
              </div>
              <p className="text-xs text-fg-subtle">
                {formatDateTime(r.startedMs)} · {formatClock(r.durationMs)} ·{" "}
                {formatBytes(r.sizeBytes)}
                {r.owner !== null && ` · ${r.owner}`}
              </p>
              {endReason(r) !== null && <p className="text-xs text-fg-muted">{endReason(r)}</p>}
              {playing === r.id && r.kind === "audio" && (
                <audio
                  controls
                  autoPlay
                  preload="metadata"
                  src={fileUrl(r.id)}
                  className="h-9 w-full"
                  aria-label={`Player for ${r.title}`}
                />
              )}
              <div className="flex flex-wrap items-center gap-1.5">
                {r.kind === "audio" && (
                  <Button
                    size="sm"
                    variant="secondary"
                    icon={playing === r.id ? <StopIcon size={12} /> : <PlayIcon size={12} />}
                    onClick={() => setPlaying(playing === r.id ? null : r.id)}
                  >
                    {playing === r.id ? "Close" : "Play"}
                  </Button>
                )}
                <a
                  href={fileUrl(r.id, { download: true })}
                  download
                  className="focus-ring inline-flex h-7 items-center gap-1.5 rounded-md border border-border-strong bg-surface-1 px-2.5 text-xs font-medium hover:bg-surface-2"
                >
                  <DownloadIcon size={12} />
                  {r.kind === "iq" ? "Data" : "Download"}
                </a>
                {r.kind === "iq" && (
                  <a
                    href={fileUrl(r.id, { part: "meta" })}
                    download
                    className="focus-ring inline-flex h-7 items-center rounded-md border border-border-strong bg-surface-1 px-2.5 text-xs font-medium hover:bg-surface-2"
                  >
                    SigMF meta
                  </a>
                )}
                {canManage && (
                  <>
                    {isRunning(r) && (
                      <Button
                        size="sm"
                        variant="danger"
                        icon={<StopIcon size={12} />}
                        disabled={busy}
                        onClick={() => void act("Cannot stop", () => api.stop(r.id))}
                      >
                        Stop
                      </Button>
                    )}
                    <IconButton
                      label={r.keep ? "Allow automatic cleanup" : "Keep (protect from cleanup)"}
                      pressed={r.keep}
                      disabled={busy}
                      onClick={() =>
                        void act("Cannot change Keep", () => api.update(r.id, { keep: !r.keep }))
                      }
                    >
                      <StarIcon size={14} className={r.keep ? "text-warn" : undefined} />
                    </IconButton>
                    <IconButton
                      label="Rename"
                      disabled={busy}
                      onClick={() => {
                        setDraft(r.title);
                        setRenaming(r.id);
                      }}
                    >
                      <PencilIcon size={14} />
                    </IconButton>
                    <IconButton
                      label="Delete"
                      disabled={busy || isRunning(r)}
                      onClick={() => setToDelete(r)}
                    >
                      <TrashIcon size={14} />
                    </IconButton>
                  </>
                )}
                {!canManage && r.keep && <Badge>kept</Badge>}
              </div>
            </li>
          ))}
        </ul>
        {list.hasMore && (
          <div className="p-3">
            <Button variant="secondary" size="sm" onClick={list.more}>
              Load more
            </Button>
          </div>
        )}
      </div>
      {toDelete !== null && (
        <ConfirmDialog
          title="Delete recording"
          confirmLabel="Delete"
          confirmVariant="danger"
          busy={busy}
          onCancel={() => setToDelete(null)}
          onConfirm={() => {
            const r = toDelete;
            setToDelete(null);
            void act("Cannot delete", () => api.remove(r.id));
          }}
        >
          “{toDelete.title || `Recording ${toDelete.id}`}” and its file are removed for good.
        </ConfirmDialog>
      )}
    </div>
  );
}
