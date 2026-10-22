import { useCallback, useEffect, useRef, useState, type FormEvent } from "react";
import { formatFrequency } from "../spectrum/freq";
import { formatRate } from "../spectrum/TuningControls";
import { ApiError } from "../spectrum/api";
import type { StateDoc } from "../state/doc";
import { lockedBy, onlyUserLock } from "../state/locks";
import {
  Alert,
  BookmarkIcon,
  Button,
  ConfirmDialog,
  Dialog,
  EmptyState,
  IconButton,
  LockIcon,
  PencilIcon,
  TextField,
  TrashIcon,
  toasts,
  type ToastInput,
} from "../ui";
import { buildProfileData, type Profile, type ProfileApi } from "./api";
import { plural, relativeTime } from "./format";

interface Props {
  api: ProfileApi;
  device: string;
  /** Newest confirmed state document; null while unknown (saving is disabled). */
  doc: StateDoc | null;
  /** A profile load answered with the new device state. */
  onState: (doc: StateDoc) => void;
  /** Drop the user lock (used by "Unlock and load"); resolves true on success. */
  onUnlock: () => Promise<boolean>;
  notify?: (toast: ToastInput) => void;
}

type Dialogs =
  | { kind: "save"; name?: string }
  | { kind: "overwrite"; name: string; existing: Profile }
  | { kind: "rename"; profile: Profile }
  | { kind: "delete"; profile: Profile }
  | { kind: "locked"; profile: Profile };

const NAME_MAX = 100;

function errorText(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

const sameName = (a: string, b: string) => a.trim().toLowerCase() === b.trim().toLowerCase();

/** Profiles section: list, load, save current, rename, delete. */
export function ProfilesPanel({ api, device, doc, onState, onUnlock, notify }: Props) {
  const push = notify ?? ((t: ToastInput) => void toasts.push(t));
  const [profiles, setProfiles] = useState<Profile[] | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [dialog, setDialog] = useState<Dialogs | null>(null);
  const [busy, setBusy] = useState(false);
  const alive = useRef(true);
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);

  const refresh = useCallback(async (): Promise<Profile[] | null> => {
    try {
      const list = await api.list();
      if (!alive.current) return null;
      setProfiles(list);
      setLoadError(null);
      return list;
    } catch (e) {
      if (alive.current) setLoadError(errorText(e));
      return null;
    }
  }, [api]);

  useEffect(() => {
    let cancelled = false;
    api
      .list()
      .then((list) => {
        if (cancelled) return;
        setProfiles(list);
        setLoadError(null);
      })
      .catch((e: unknown) => {
        if (!cancelled) setLoadError(errorText(e));
      });
    return () => {
      cancelled = true;
    };
  }, [api]);

  const locks = doc?.device.locks ?? [];

  const doLoad = async (p: Profile) => {
    setBusy(true);
    try {
      const next = await api.load(device, p.id);
      if (!alive.current) return;
      onState(next);
      push({ tone: "success", title: `Loaded “${p.name}”`, key: "profile" });
    } catch (e) {
      if (e instanceof ApiError && e.code === "locked") {
        push({
          tone: "warn",
          title: "Tuning is locked",
          body: `${e.holders?.length ? `${lockedBy(e.holders)}. ` : ""}“${p.name}” changes the centre or sample rate. Unlock first.`,
          key: "profile",
        });
      } else {
        push({
          tone: "danger",
          title: `Could not load “${p.name}”`,
          body: errorText(e),
          key: "profile",
        });
      }
    } finally {
      if (alive.current) setBusy(false);
    }
  };

  const startLoad = (p: Profile) => {
    const d = doc?.device;
    const moves = d !== undefined && (p.centerHz !== d.centerHz || p.sampleRate !== d.sampleRate);
    if (locks.length > 0 && moves) setDialog({ kind: "locked", profile: p });
    else void doLoad(p);
  };

  const unlockAndLoad = async (p: Profile) => {
    setBusy(true);
    const ok = await onUnlock();
    if (!alive.current) return;
    setBusy(false);
    setDialog(null);
    if (ok) await doLoad(p);
  };

  const remove = async (p: Profile) => {
    setBusy(true);
    try {
      await api.remove(p.id);
      push({ tone: "info", title: `Deleted “${p.name}”`, key: "profile" });
    } catch (e) {
      // 404: someone else deleted it already; the refresh below shows the truth.
      if (!(e instanceof ApiError && e.status === 404)) {
        push({ tone: "danger", title: "Could not delete the profile", body: errorText(e) });
      }
    }
    if (!alive.current) return;
    setBusy(false);
    setDialog(null);
    await refresh();
  };

  const overwrite = async (name: string, existing: Profile) => {
    if (!doc) return;
    setBusy(true);
    try {
      await api.update(existing.id, existing.name, buildProfileData(doc, existing.data));
      push({ tone: "success", title: `Saved “${name}”`, key: "profile" });
      setDialog(null);
    } catch (e) {
      push({ tone: "danger", title: "Could not overwrite the profile", body: errorText(e) });
    }
    if (!alive.current) return;
    setBusy(false);
    await refresh();
  };

  return (
    <div className="flex flex-col gap-3" role="group" aria-label="Profiles">
      <Button
        icon={<BookmarkIcon />}
        disabled={!doc}
        title="Store the current centre, rate, gain and VFOs as a profile"
        onClick={() => setDialog({ kind: "save" })}
      >
        Save current as…
      </Button>
      {loadError !== null && (
        <Alert tone="warn">
          <p>Profiles are unavailable: {loadError}</p>
          <Button size="sm" className="mt-2" onClick={() => void refresh()}>
            Retry
          </Button>
        </Alert>
      )}
      {profiles === null && loadError === null && (
        <p className="text-xs text-fg-subtle">Loading…</p>
      )}
      {profiles !== null && profiles.length === 0 && (
        <EmptyState title="No profiles yet">
          Tune the receiver the way you like it, then press “Save current as…”.
        </EmptyState>
      )}
      {profiles !== null && profiles.length > 0 && (
        <ul className="flex flex-col gap-1.5" aria-label="Profile list">
          {profiles.map((p) => (
            <li
              key={p.id}
              className="flex items-center gap-1 rounded-md border border-border bg-surface-1 py-1.5 pr-1 pl-2.5"
            >
              <div className="min-w-0 flex-1">
                <p className="flex items-center gap-1 text-sm font-medium text-fg">
                  <span className="truncate">{p.name}</span>
                  {p.lock && (
                    <span title="Locks the tuning when loaded" className="shrink-0 text-warn">
                      <LockIcon size={12} />
                      <span className="sr-only">locks tuning</span>
                    </span>
                  )}
                </p>
                <p className="truncate text-xs text-fg-muted num">
                  {formatFrequency(p.centerHz).slice(0, -4)} MHz · {formatRate(p.sampleRate)} ·{" "}
                  {plural(p.vfoCount, "VFO")}
                  {p.updatedAt && ` · ${relativeTime(p.updatedAt)}`}
                </p>
              </div>
              <Button
                size="sm"
                disabled={busy}
                aria-label={`Load profile ${p.name}`}
                onClick={() => startLoad(p)}
              >
                Load
              </Button>
              <IconButton
                label={`Rename profile ${p.name}`}
                size="sm"
                disabled={busy}
                onClick={() => setDialog({ kind: "rename", profile: p })}
              >
                <PencilIcon size={14} />
              </IconButton>
              <IconButton
                label={`Delete profile ${p.name}`}
                size="sm"
                disabled={busy}
                onClick={() => setDialog({ kind: "delete", profile: p })}
              >
                <TrashIcon size={14} />
              </IconButton>
            </li>
          ))}
        </ul>
      )}

      {dialog?.kind === "save" && (
        <NameDialog
          title="Save current as…"
          submitLabel="Save"
          initial={dialog.name ?? ""}
          onCancel={() => setDialog(null)}
          onSubmit={async (name) => {
            try {
              await api.saveCurrent(device, name);
            } catch (e) {
              if (e instanceof ApiError && e.code === "conflict") {
                const list = profiles ?? (await refresh()) ?? [];
                const existing = list.find((p) => sameName(p.name, name));
                if (existing && alive.current) {
                  setDialog({ kind: "overwrite", name, existing });
                  return null;
                }
              }
              return errorText(e);
            }
            push({ tone: "success", title: `Saved “${name}”`, key: "profile" });
            setDialog(null);
            await refresh();
            return null;
          }}
        />
      )}
      {dialog?.kind === "overwrite" && (
        <ConfirmDialog
          title="Overwrite profile?"
          confirmLabel="Overwrite"
          confirmVariant="danger"
          busy={busy}
          onCancel={() => setDialog({ kind: "save", name: dialog.name })}
          onConfirm={() => void overwrite(dialog.name, dialog.existing)}
        >
          A profile named “{dialog.existing.name}” already exists. Replace it with the current
          receiver settings?
        </ConfirmDialog>
      )}
      {dialog?.kind === "rename" && (
        <NameDialog
          title="Rename profile"
          submitLabel="Rename"
          initial={dialog.profile.name}
          onCancel={() => setDialog(null)}
          onSubmit={async (name) => {
            const p = dialog.profile;
            if (name !== p.name) {
              try {
                await api.update(p.id, name, p.data);
              } catch (e) {
                return e instanceof ApiError && e.code === "conflict"
                  ? "A profile with this name already exists."
                  : errorText(e);
              }
            }
            setDialog(null);
            await refresh();
            return null;
          }}
        />
      )}
      {dialog?.kind === "delete" && (
        <ConfirmDialog
          title="Delete profile?"
          confirmLabel="Delete"
          confirmVariant="danger"
          busy={busy}
          onCancel={() => setDialog(null)}
          onConfirm={() => void remove(dialog.profile)}
        >
          “{dialog.profile.name}” will be removed permanently. The receiver is not changed.
        </ConfirmDialog>
      )}
      {dialog?.kind === "locked" && (
        <ConfirmDialog
          title="Tuning is locked"
          confirmLabel="Unlock and load"
          busy={busy}
          onCancel={() => setDialog(null)}
          onConfirm={onlyUserLock(locks) ? () => void unlockAndLoad(dialog.profile) : undefined}
        >
          <p>
            {lockedBy(locks)}. “{dialog.profile.name}” would move the centre to{" "}
            <span className="num">{formatFrequency(dialog.profile.centerHz).slice(0, -4)} MHz</span>{" "}
            at {formatRate(dialog.profile.sampleRate)}, which the lock does not allow.
          </p>
          {!onlyUserLock(locks) && (
            <p className="mt-2">It becomes loadable when the other holders finish.</p>
          )}
        </ConfirmDialog>
      )}
    </div>
  );
}

interface NameDialogProps {
  title: string;
  submitLabel: string;
  initial: string;
  onCancel: () => void;
  /** Resolves with an error text to show inline, or null when done. */
  onSubmit: (name: string) => Promise<string | null>;
}

function NameDialog({ title, submitLabel, initial, onCancel, onSubmit }: NameDialogProps) {
  const [name, setName] = useState(initial);
  const [error, setError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    const trimmed = name.trim();
    if (trimmed === "") {
      setError("Enter a name.");
      return;
    }
    setPending(true);
    setError(null);
    const result = await onSubmit(trimmed);
    setPending(false);
    if (result !== null) setError(result);
  };

  return (
    <Dialog title={title} onClose={onCancel}>
      <form onSubmit={(e) => void submit(e)} className="flex flex-col gap-4" noValidate>
        <TextField
          label="Profile name"
          value={name}
          maxLength={NAME_MAX}
          error={error}
          autoComplete="off"
          onChange={(e) => {
            setName(e.target.value);
            setError(null);
          }}
        />
        <div className="flex justify-end gap-2">
          <Button variant="secondary" onClick={onCancel}>
            Cancel
          </Button>
          <Button type="submit" variant="primary" disabled={pending}>
            {submitLabel}
          </Button>
        </div>
      </form>
    </Dialog>
  );
}
