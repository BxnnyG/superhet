// Bookmark list plus the state of the add/edit dialog, as one external store. Plain TS so the
// spectrum overlay and the React panels share it; every change re-reads the list.

import { createBookmarkApi, type BookmarkApi } from "./api";
import { sortBookmarks, type Bookmark, type BookmarkInput } from "./model";

export type EditorTarget =
  { kind: "create"; draft: BookmarkInput } | { kind: "edit"; bookmark: Bookmark };

export interface BookmarkSnapshot {
  bookmarks: readonly Bookmark[];
  /** The first read finished (successfully or not). */
  loaded: boolean;
  error: string | null;
  editor: EditorTarget | null;
}

const errorText = (e: unknown) => (e instanceof Error ? e.message : String(e));

export class BookmarkStore {
  private snap: BookmarkSnapshot = { bookmarks: [], loaded: false, error: null, editor: null };
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
      const list = await this.api.list();
      if (seq === this.seq) this.set({ bookmarks: sortBookmarks(list), loaded: true, error: null });
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

  /** Bulk import; resolves with the number created or an error text. */
  async bulk(
    items: readonly BookmarkInput[],
    replace: boolean,
  ): Promise<{ created: number } | { error: string }> {
    let created = 0;
    const error = await this.change(async () => {
      created = await this.api.bulk(items, replace);
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
