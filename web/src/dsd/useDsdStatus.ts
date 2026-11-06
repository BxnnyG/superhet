// Polls the status document of one DMR / dPMR / NXDN instance (calls on the air, is dsd-fme there)
// while a panel shows it.
//   GET /api/v1/plugin-instances/{id}/status -> { status: { tool, now, last, calls }, ... }

import { useEffect, useState } from "react";
import { apiFetch } from "../auth/session";
import { httpError, readJson } from "../spectrum/api";
import { EMPTY_STATUS, parseStatus, type DsdStatus } from "./model";

export const DSD_POLL_MS = 1000;

export type FetchStatus = (instanceId: string, signal: AbortSignal) => Promise<DsdStatus>;

export const fetchDsdStatus: FetchStatus = async (instanceId, signal) => {
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
export function useDsdStatus(
  instanceId: string | null,
  fetchStatus: FetchStatus = fetchDsdStatus,
): { status: DsdStatus; stale: boolean } {
  const [state, setState] = useState<{ id: string | null; status: DsdStatus; stale: boolean }>({
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
    const timer = setInterval(() => void poll(), DSD_POLL_MS);
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
