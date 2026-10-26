import { useState, useSyncExternalStore, type FormEvent } from "react";
import {
  Alert,
  ArrowLeftIcon,
  ArrowRightIcon,
  Button,
  Dialog,
  IconButton,
  PencilIcon,
  TextField,
  TrashIcon,
} from "../ui";
import { GROUP_NAME_MAX, type BookmarkGroup } from "./model";
import type { BookmarkStore } from "./store";

interface Props {
  store: BookmarkStore;
  onClose: () => void;
}

/** Create, rename, reorder and delete bookmark groups (folders). Bookmarks of a deleted group stay. */
export function GroupsDialog({ store, onClose }: Props) {
  const { groups, bookmarks } = useSyncExternalStore(store.subscribe, store.getSnapshot);
  const [name, setName] = useState("");
  const [editing, setEditing] = useState<{ id: string; name: string } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const run = async (job: () => Promise<string | null>): Promise<boolean> => {
    setBusy(true);
    setError(null);
    const e = await job();
    setBusy(false);
    if (e !== null) setError(e);
    return e === null;
  };

  const add = async (e: FormEvent) => {
    e.preventDefault();
    const trimmed = name.trim();
    if (trimmed === "") return;
    if (await run(() => store.createGroup(trimmed))) setName("");
  };

  const rename = async (e: FormEvent) => {
    e.preventDefault();
    if (editing === null) return;
    const trimmed = editing.name.trim();
    if (trimmed === "") return;
    if (await run(() => store.renameGroup(editing.id, trimmed))) setEditing(null);
  };

  const count = (g: BookmarkGroup) => bookmarks.filter((b) => b.groupId === g.id).length;

  return (
    <Dialog title="Bookmark groups" onClose={onClose}>
      <div className="flex flex-col gap-3 text-sm">
        <form onSubmit={(e) => void add(e)} className="flex items-end gap-2" noValidate>
          <TextField
            label="New group"
            value={name}
            maxLength={GROUP_NAME_MAX}
            autoComplete="off"
            className="flex-1"
            onChange={(e) => setName(e.target.value)}
          />
          <Button type="submit" variant="primary" disabled={busy || name.trim() === ""}>
            Add
          </Button>
        </form>
        {groups.length === 0 && (
          <p className="text-xs text-fg-muted">
            No groups yet. Groups are folders in the list and in exports; a bookmark is in one group
            at most.
          </p>
        )}
        <ul className="flex flex-col gap-1" aria-label="Groups">
          {groups.map((g, n) => (
            <li key={g.id} className="flex items-center gap-1">
              {editing?.id === g.id ? (
                <form onSubmit={(e) => void rename(e)} className="flex flex-1 items-end gap-2">
                  <TextField
                    label={`Rename ${g.name}`}
                    value={editing.name}
                    maxLength={GROUP_NAME_MAX}
                    autoComplete="off"
                    className="flex-1"
                    onChange={(e) => setEditing({ id: g.id, name: e.target.value })}
                  />
                  <Button type="submit" variant="primary" disabled={busy}>
                    Save
                  </Button>
                  <Button onClick={() => setEditing(null)}>Cancel</Button>
                </form>
              ) : (
                <>
                  <span className="min-w-0 flex-1 truncate text-fg">
                    {g.name} <span className="text-xs text-fg-subtle">({count(g)})</span>
                  </span>
                  <IconButton
                    label={`Move ${g.name} up`}
                    size="sm"
                    disabled={busy || n === 0}
                    onClick={() => void run(() => store.moveGroup(g.id, -1))}
                  >
                    <ArrowLeftIcon size={14} className="rotate-90" />
                  </IconButton>
                  <IconButton
                    label={`Move ${g.name} down`}
                    size="sm"
                    disabled={busy || n === groups.length - 1}
                    onClick={() => void run(() => store.moveGroup(g.id, 1))}
                  >
                    <ArrowRightIcon size={14} className="rotate-90" />
                  </IconButton>
                  <IconButton
                    label={`Rename group ${g.name}`}
                    size="sm"
                    onClick={() => setEditing({ id: g.id, name: g.name })}
                  >
                    <PencilIcon size={14} />
                  </IconButton>
                  <IconButton
                    label={`Delete group ${g.name}`}
                    size="sm"
                    disabled={busy}
                    onClick={() => void run(() => store.removeGroup(g.id))}
                  >
                    <TrashIcon size={14} />
                  </IconButton>
                </>
              )}
            </li>
          ))}
        </ul>
        {error !== null && <Alert tone="danger">{error}</Alert>}
        <p className="text-xs text-fg-subtle">Deleting a group keeps its bookmarks (ungrouped).</p>
        <div className="flex justify-end">
          <Button onClick={onClose}>Done</Button>
        </div>
      </div>
    </Dialog>
  );
}
