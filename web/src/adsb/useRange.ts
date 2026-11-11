import { useCallback, useEffect, useState } from "react";
import { describeHttpError } from "../http";
import type { AdsbApi } from "./api";
import type { RangeData } from "./range";

/** How often the statistics are refreshed while they are on screen (the plugin reports per minute). */
export const RANGE_REFRESH_MS = 60_000;

export interface RangeState {
  data: RangeData | null;
  error: string | null;
  loading: boolean;
  reload: () => void;
}

/** Load the range statistics of `device` for the last `hours`, refreshed once a minute. */
export function useRange(api: AdsbApi, device: string, hours: number, active = true): RangeState {
  const [state, setState] = useState<{
    key: string;
    data: RangeData | null;
    error: string | null;
  }>({ key: "", data: null, error: null });
  const [tick, setTick] = useState(0);
  const key = `${device}|${hours}`;

  useEffect(() => {
    if (!active) return;
    const ctrl = new AbortController();
    const run = () => {
      api.range(device, hours, ctrl.signal).then(
        (data) => {
          if (!ctrl.signal.aborted) setState({ key, data, error: null });
        },
        (e: unknown) => {
          if (ctrl.signal.aborted) return;
          setState((s) => ({
            key,
            data: s.key === key ? s.data : null,
            error: describeHttpError(e),
          }));
        },
      );
    };
    run();
    const id = setInterval(run, RANGE_REFRESH_MS);
    return () => {
      ctrl.abort();
      clearInterval(id);
    };
  }, [api, device, hours, active, key, tick]);

  const reload = useCallback(() => setTick((t) => t + 1), []);
  const current = state.key === key;
  const data = current ? state.data : null;
  const error = current ? state.error : null;
  return { data, error, loading: data === null && error === null, reload };
}
