// Loads the extras of the selected aircraft for the panel: online details (photo, route, full
// type name; only when an admin enabled them) and the "seen before" history (a local query).
// Both wait a moment so clicking quickly through the table does not fire a request per row, and
// answers are kept per aircraft for a while so reselecting is instant. The state lives in a small
// external store (like `InfoCache`); the hooks only subscribe to it.

import { useCallback, useEffect, useSyncExternalStore } from "react";
import { describeHttpError } from "../http";
import type { AdsbApi } from "./api";
import type { OnlineReply, SeenHistory } from "./online";

export type Load<T> =
  { status: "loading" } | { status: "error"; message: string } | { status: "ready"; data: T };

/** Wait this long after a selection before asking anything. */
export const SETTLE_MS = 250;
/** Answers are reused this long. */
export const KEEP_MS = 10 * 60_000;

const LOADING: Load<never> = { status: "loading" };

interface Entry<T> {
  load: Load<T>;
  /** When a ready answer arrived, ms since epoch. */
  at: number;
}

/** One kind of lookup, keyed by aircraft. */
export class Loader<T> {
  private readonly entries = new Map<string, Entry<T>>();
  private readonly listeners = new Set<() => void>();

  /**
   * @param keep Whether an answer is worth keeping (a failed or paused service is asked again).
   */
  constructor(private readonly keep: (data: T) => boolean = () => true) {}

  readonly subscribe = (fn: () => void): (() => void) => {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  };

  snapshot(key: string): Load<T> {
    return this.entries.get(key)?.load ?? LOADING;
  }

  private set(key: string, entry: Entry<T> | null): void {
    if (entry === null) this.entries.delete(key);
    else this.entries.set(key, entry);
    for (const fn of [...this.listeners]) fn();
  }

  /** Forget everything (the admin changed what may be asked). */
  clear(): void {
    this.entries.clear();
    for (const fn of [...this.listeners]) fn();
  }

  /**
   * Make sure `key` is loaded: a kept answer is used as it is, anything else is fetched after
   * [`SETTLE_MS`]. Returns a function that cancels a request that has not finished.
   */
  request(key: string, fetcher: (signal: AbortSignal) => Promise<T>, force = false): () => void {
    const have = this.entries.get(key);
    if (!force && have?.load.status === "ready" && Date.now() - have.at < KEEP_MS) {
      return () => undefined;
    }
    if (have?.load.status !== "loading") this.set(key, { load: LOADING, at: 0 });
    const ctrl = new AbortController();
    const timer = setTimeout(() => {
      fetcher(ctrl.signal).then(
        (data) => {
          if (ctrl.signal.aborted) return;
          if (this.keep(data)) this.set(key, { load: { status: "ready", data }, at: Date.now() });
          // An answer that is not kept is still shown, but asked again next time.
          else this.set(key, { load: { status: "ready", data }, at: 0 });
        },
        (e: unknown) => {
          if (ctrl.signal.aborted) return;
          this.set(key, { load: { status: "error", message: describeHttpError(e) }, at: 0 });
        },
      );
    }, SETTLE_MS);
    return () => {
      clearTimeout(timer);
      ctrl.abort();
    };
  }
}

/** An answer worth keeping: the sources answered (or have nothing), not failed or paused. */
const settled = (r: OnlineReply): boolean =>
  r.adsbdb !== "unavailable" &&
  r.adsbdb !== "rate_limited" &&
  r.planespotters !== "unavailable" &&
  r.planespotters !== "rate_limited";

const onlineLoaders = new WeakMap<AdsbApi, Loader<OnlineReply>>();
const historyLoaders = new WeakMap<AdsbApi, Loader<SeenHistory | null>>();

function loaderOf<T>(
  map: WeakMap<AdsbApi, Loader<T>>,
  api: AdsbApi,
  keep?: (data: T) => boolean,
): Loader<T> {
  let l = map.get(api);
  if (l === undefined) {
    l = new Loader<T>(keep);
    map.set(api, l);
  }
  return l;
}

/** Forget the online answers kept for `api` (after an admin changed the switches). */
export function forgetExtras(api: AdsbApi): void {
  onlineLoaders.get(api)?.clear();
}

/** Online details of one aircraft; `retry` asks again, bypassing the kept answer. */
export function useOnline(
  api: AdsbApi,
  icao: string,
  callsign: string | null,
): { load: Load<OnlineReply>; retry: () => void } {
  const loader = loaderOf(onlineLoaders, api, settled);
  const key = `${icao}|${callsign ?? ""}`;
  const load = useSyncExternalStore(loader.subscribe, () => loader.snapshot(key));
  useEffect(
    () => loader.request(key, (signal) => api.online(icao, callsign, signal)),
    [loader, key, api, icao, callsign],
  );
  const retry = useCallback(() => {
    loader.request(key, (signal) => api.online(icao, callsign, signal), true);
  }, [loader, key, api, icao, callsign]);
  return { load, retry };
}

/** What the decoder history says about one aircraft. */
export function useHistory(api: AdsbApi, icao: string): Load<SeenHistory | null> {
  const loader = loaderOf(historyLoaders, api);
  const load = useSyncExternalStore(loader.subscribe, () => loader.snapshot(icao));
  useEffect(() => loader.request(icao, (signal) => api.history(icao, signal)), [loader, icao, api]);
  return load;
}
