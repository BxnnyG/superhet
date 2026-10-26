import { useCallback, useEffect, useState } from "react";
import { describeHttpError } from "../http";

export interface Loaded<T> {
  data: T | null;
  error: string | null;
  /** The first load of this `load` function has not finished. */
  loading: boolean;
  /** Fetch again now (keeps showing the old data meanwhile). */
  reload: () => void;
}

/**
 * Runs `load` on mount and whenever the (memoised) function changes, optionally again every
 * `refreshMs`. Stale answers of an older `load` are discarded; data stays visible during reloads.
 */
export function useLoad<T>(load: (signal: AbortSignal) => Promise<T>, refreshMs = 0): Loaded<T> {
  const [tick, setTick] = useState(0);
  const [state, setState] = useState<{ owner: unknown; data: T | null; error: string | null }>({
    owner: null,
    data: null,
    error: null,
  });

  useEffect(() => {
    const ctrl = new AbortController();
    const run = () => {
      load(ctrl.signal).then(
        (data) => {
          if (!ctrl.signal.aborted) setState({ owner: load, data, error: null });
        },
        (e: unknown) => {
          if (ctrl.signal.aborted) return;
          setState((s) => ({
            owner: load,
            data: s.owner === load ? s.data : null,
            error: describeHttpError(e),
          }));
        },
      );
    };
    run();
    const id = refreshMs > 0 ? setInterval(run, refreshMs) : undefined;
    return () => {
      ctrl.abort();
      if (id !== undefined) clearInterval(id);
    };
  }, [load, refreshMs, tick]);

  const reload = useCallback(() => setTick((t) => t + 1), []);
  const current = state.owner === load;
  const data = current ? state.data : null;
  const error = current ? state.error : null;
  return { data, error, loading: data === null && error === null, reload };
}
