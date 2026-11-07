// Polls the status document of one TETRA instance (cell, calls on the air, is tetra-rx / the ETSI
// codec there) while a panel shows it.
//   GET /api/v1/plugin-instances/{id}/status -> { status: { tool, codec, signal, cell, now, ... } }

import { useEffect, useState } from "react";
import { apiFetch } from "../auth/session";
import { httpError, readJson } from "../spectrum/api";
import { EMPTY_STATUS, parseStatus, type TetraStatus } from "./model";

export const TETRA_POLL_MS = 1000;

export type FetchStatus = (instanceId: string, signal: AbortSignal) => Promise<TetraStatus>;

export const fetchTetraStatus: FetchStatus = async (instanceId, signal) => {
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
export function useTetraStatus(
  instanceId: string | null,
  fetchStatus: FetchStatus = fetchTetraStatus,
): { status: TetraStatus; stale: boolean } {
  const [state, setState] = useState<{ id: string | null; status: TetraStatus; stale: boolean }>({
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
    const timer = setInterval(() => void poll(), TETRA_POLL_MS);
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
