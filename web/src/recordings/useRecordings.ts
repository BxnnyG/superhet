import { useCallback, useEffect, useRef, useState } from "react";
import { type ListQuery, type Recording, type RecordingsApi, recordingsApi } from "./api";

/** Refresh interval while the list is mounted (running recordings grow). */
export const POLL_MS = 3000;

export interface RecordingList {
  items: Recording[];
  loading: boolean;
  error: string | null;
  hasMore: boolean;
  reload: () => void;
  more: () => void;
}

/** A polled page of recordings; `query` changes restart it. */
export function useRecordings(
  query: ListQuery,
  opts: { api?: RecordingsApi; pollMs?: number; enabled?: boolean } = {},
): RecordingList {
  const { api = recordingsApi, pollMs = POLL_MS, enabled = true } = opts;
  const [items, setItems] = useState<Recording[]>([]);
  const [loading, setLoading] = useState(enabled);
  const [error, setError] = useState<string | null>(null);
  const [next, setNext] = useState<number | null>(null);
  const [tick, setTick] = useState(0);
  const key = JSON.stringify(query);
  // Older pages loaded with "more" stay until the query changes.
  const older = useRef<Recording[]>([]);

  useEffect(() => {
    older.current = [];
  }, [key]);

  useEffect(() => {
    if (!enabled) return;
    let alive = true;
    const load = async () => {
      try {
        const page = await api.list(JSON.parse(key) as ListQuery);
        if (!alive) return;
        const ids = new Set(page.items.map((r) => r.id));
        setItems([...page.items, ...older.current.filter((r) => !ids.has(r.id))]);
        if (older.current.length === 0) setNext(page.nextBefore);
        setError(null);
      } catch (e) {
        if (alive) setError(e instanceof Error ? e.message : String(e));
      } finally {
        if (alive) setLoading(false);
      }
    };
    void load();
    const timer = pollMs > 0 ? window.setInterval(() => void load(), pollMs) : undefined;
    return () => {
      alive = false;
      if (timer !== undefined) window.clearInterval(timer);
    };
  }, [api, key, pollMs, enabled, tick]);

  const reload = useCallback(() => setTick((t) => t + 1), []);
  const more = useCallback(() => {
    if (next === null) return;
    void api
      .list({ ...(JSON.parse(key) as ListQuery), before: next })
      .then((page) => {
        older.current = [...older.current, ...page.items];
        setNext(page.nextBefore);
        setItems((cur) => [...cur, ...page.items]);
      })
      .catch((e: unknown) => setError(e instanceof Error ? e.message : String(e)));
  }, [api, key, next]);

  return { items, loading, error, hasMore: next !== null, reload, more };
}
