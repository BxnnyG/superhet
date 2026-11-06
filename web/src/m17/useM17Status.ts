// Polls the status document of one M17 instance (who is on the air) while a panel shows it.
//   GET /api/v1/plugin-instances/{id}/status -> { status: { now, last, calls }, ... }

import { useEffect, useState } from "react";
import { apiFetch } from "../auth/session";
import { httpError, readJson } from "../spectrum/api";
import { EMPTY_STATUS, parseStatus, type M17Status } from "./model";

export const M17_POLL_MS = 1000;

export type FetchStatus = (instanceId: string, signal: AbortSignal) => Promise<M17Status>;

export const fetchM17Status: FetchStatus = async (instanceId, signal) => {
  const res = await apiFetch(`/api/v1/plugin-instances/${encodeURIComponent(instanceId)}/status`, {
    signal,
  });
  const body = await readJson(res);
  if (!res.ok) throw httpError(res, body);
  return parseStatus(body);
};

/**
 * The plugin's own picture of the channel, refreshed every second while the tab is visible.
 * `status` keeps the last good document when a poll fails; `stale` says the plugin could not be
 * reached lately, so the panel does not claim a call is still going on.
 */
export function useM17Status(
  instanceId: string | null,
  fetchStatus: FetchStatus = fetchM17Status,
): { status: M17Status; stale: boolean } {
  const [state, setState] = useState<{ id: string | null; status: M17Status; stale: boolean }>({
    id: null,
    status: EMPTY_STATUS,
    stale: false,
  });
  useEffect(() => {
    if (instanceId === null) return;
    let cancelled = false;
    const abort = new AbortController();
    let busy = false;
    const poll = async () => {
      if (busy || document.hidden) return;
      busy = true;
      try {
        const status = await fetchStatus(instanceId, abort.signal);
        if (!cancelled) setState({ id: instanceId, status, stale: false });
      } catch {
        if (!cancelled) setState((s) => ({ ...s, id: instanceId, stale: true }));
      } finally {
        busy = false;
      }
    };
    void poll();
    const timer = setInterval(() => void poll(), M17_POLL_MS);
    return () => {
      cancelled = true;
      abort.abort();
      clearInterval(timer);
    };
  }, [instanceId, fetchStatus]);
  // A different instance than the one the last document belongs to starts from scratch.
  if (instanceId === null || state.id !== instanceId) return { status: EMPTY_STATUS, stale: false };
  return { status: state.status, stale: state.stale };
}
