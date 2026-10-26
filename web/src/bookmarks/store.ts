// Bookmark list plus the state of the add/edit dialog, as one external store. Plain TS so the
// spectrum overlay and the React panels share it; every change re-reads the list.

import { createBookmarkApi, type BookmarkApi } from "./api";
import { sortBookmarks, type Bookmark, type BookmarkGroup, type BookmarkInput } from "./model";

export type EditorTarget =
  { kind: "create"; draft: BookmarkInput } | { kind: "edit"; bookmark: Bookmark };

/** A bookmark to import; `groupName` is created as a group when it does not exist yet. */
export type ImportItem = BookmarkInput & { groupName?: string | null };

export interface BookmarkSnapshot {
  bookmarks: readonly Bookmark[];
  /** In display order. */
  groups: readonly BookmarkGroup[];
  /** The first read finished (successfully or not). */
  loaded: boolean;
  error: string | null;
  editor: EditorTarget | null;
}

const errorText = (e: unknown) => (e instanceof Error ? e.message : String(e));

export class BookmarkStore {
  private snap: BookmarkSnapshot = {
    bookmarks: [],
    groups: [],
    loaded: false,
    error: null,
    editor: null,
  };
  private readonly listeners = new Set<() => void>();
  private seq = 0;

  constructor(private readonly api: BookmarkApi = createBookmarkApi()) {}

  subscribe = (fn: () => void): (() => void) => {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  };

  getSnapshot = (): BookmarkSnapshot => this.snap;

  private set(patch: Partial<BookmarkSnapshot>): void {
    this.snap = { ...this.snap, ...patch };
    for (const fn of this.listeners) fn();
  }

  /** Re-read the list; only the newest read may land. */
  async refresh(): Promise<void> {
    const seq = ++this.seq;
    try {
      const [list, groups] = await Promise.all([
        this.api.list(),
        // Groups are secondary: a server without them still lists its bookmarks.
        this.api.listGroups().catch((): BookmarkGroup[] => []),
      ]);
      if (seq === this.seq) {
        this.set({
          bookmarks: sortBookmarks(list),
          groups: [...groups].sort((a, b) => a.position - b.position),
          loaded: true,
          error: null,
        });
      }
    } catch (e) {
      if (seq === this.seq) this.set({ loaded: true, error: errorText(e) });
    }
  }

  /** Run a change, then re-read. Resolves with an error text or null. */
  private async change(run: () => Promise<unknown>): Promise<string | null> {
    try {
      await run();
    } catch (e) {
      await this.refresh();
      return errorText(e);
    }
    await this.refresh();
    return null;
  }

  create(input: BookmarkInput): Promise<string | null> {
    return this.change(() => this.api.create(input));
  }

  update(id: string, patch: Partial<BookmarkInput>): Promise<string | null> {
    return this.change(() => this.api.update(id, patch));
  }

  remove(id: string): Promise<string | null> {
    return this.change(() => this.api.remove(id));
  }

  createGroup(name: string): Promise<string | null> {
    return this.change(() => this.api.createGroup(name));
  }

  renameGroup(id: string, name: string): Promise<string | null> {
    return this.change(() => this.api.renameGroup(id, name));
  }

  removeGroup(id: string): Promise<string | null> {
    return this.change(() => this.api.removeGroup(id));
  }

  /** Move a group one place up (-1) or down (+1). */
  moveGroup(id: string, by: -1 | 1): Promise<string | null> {
    const ids = this.snap.groups.map((g) => g.id);
    const from = ids.indexOf(id);
    const to = from + by;
    if (from < 0 || to < 0 || to >= ids.length) return Promise.resolve(null);
    ids.splice(to, 0, ...ids.splice(from, 1));
    return this.change(() => this.api.orderGroups(ids));
  }

  /**
   * Bulk import; resolves with the number created or an error text. Groups named by the items
   * are created first (matched ignoring case).
   */
  async bulk(
    items: readonly ImportItem[],
    replace: boolean,
  ): Promise<{ created: number } | { error: string }> {
    let created = 0;
    const error = await this.change(async () => {
      const ids = new Map(this.snap.groups.map((g) => [g.name.toLowerCase(), g.id]));
      const wired: BookmarkInput[] = [];
      for (const item of items) {
        const { groupName, ...input } = item;
        const name = groupName?.trim() ?? "";
        if (name !== "") {
          let id = ids.get(name.toLowerCase());
          if (id === undefined) {
            id = (await this.api.createGroup(name)).id;
            ids.set(name.toLowerCase(), id);
          }
          input.groupId = id;
        }
        wired.push(input);
      }
      created = await this.api.bulk(wired, replace);
    });
    return error === null ? { created } : { error };
  }

  openEditor(editor: EditorTarget): void {
    this.set({ editor });
  }

  closeEditor(): void {
    if (this.snap.editor !== null) this.set({ editor: null });
  }
}
