// The decoder event log as an external store: history from REST, new events from the live feed
// (/api/v1/ws/events). The reducer (events.ts) owns all state transitions; this class only does
// the I/O and keeps the socket alive (reconnect with backoff, stops while the session is lost,
// reloads the newest page after a reconnect so nothing is missed).

import { checkSessionAfterClose, isSessionLost } from "../auth/session";
import { backoffDelay, type SocketStatus } from "../spectrum/socket";
import { createEventApi, type EventApi } from "./api";
import {
  INITIAL_LOG,
  logReducer,
  parseEvent,
  type LogAction,
  type LogFilter,
  type LogState,
} from "./events";

export interface EventSnapshot extends LogState {
  status: SocketStatus;
}

export interface EventStoreOptions {
  device: string;
  api?: EventApi;
  /** Override for tests. */
  createSocket?: (url: string) => WebSocket;
}

export function eventsUrl(device: string, loc: Pick<Location, "protocol" | "host"> = location) {
  const scheme = loc.protocol === "https:" ? "wss:" : "ws:";
  return `${scheme}//${loc.host}/api/v1/ws/events?dev=${encodeURIComponent(device)}`;
}

const errorText = (e: unknown) => (e instanceof Error ? e.message : String(e));

/** Parse one frame of the live feed; null for anything else. */
export function parseEventFrame(data: string) {
  try {
    const msg: unknown = JSON.parse(data);
    if (typeof msg !== "object" || msg === null) return null;
    const m = msg as Record<string, unknown>;
    return m.v === 1 && m.type === "decoder_event" ? parseEvent(m.event) : null;
  } catch {
    return null;
  }
}

export class DecoderEventStore {
  private log: LogState = INITIAL_LOG;
  private status: SocketStatus = "connecting";
  private snap: EventSnapshot = { ...INITIAL_LOG, status: "connecting" };
  private readonly listeners = new Set<() => void>();
  private readonly api: EventApi;
  private ws: WebSocket | null = null;
  private attempt = 0;
  private reconnectTimer: ReturnType<typeof setTimeout> | undefined;
  private closed = true;
  private abort: AbortController | null = null;

  constructor(private readonly opts: EventStoreOptions) {
    this.api = opts.api ?? createEventApi();
  }

  readonly subscribe = (fn: () => void): (() => void) => {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  };

  readonly getSnapshot = (): EventSnapshot => this.snap;

  private dispatch(action: LogAction): void {
    this.log = logReducer(this.log, action);
    this.publish();
  }

  private publish(): void {
    this.snap = { ...this.log, status: this.status };
    for (const fn of [...this.listeners]) fn();
  }

  start(): void {
    if (!this.closed) return;
    this.closed = false;
    this.attempt = 0;
    this.connect();
    void this.query(undefined);
  }

  stop(): void {
    this.closed = true;
    clearTimeout(this.reconnectTimer);
    this.abort?.abort();
    const ws = this.ws;
    this.ws = null;
    if (ws) {
      ws.onopen = ws.onmessage = ws.onclose = ws.onerror = null;
      ws.close();
    }
  }

  /** Change the filter and load its first page. */
  setFilter(filter: LogFilter): void {
    void this.query(filter);
  }

  /** Load the first page again, keeping what is shown. */
  refresh(): void {
    void this.query(undefined);
  }

  pause(): void {
    this.dispatch({ type: "pause" });
  }

  resume(): void {
    this.dispatch({ type: "resume" });
  }

  /** Delete the whole history on the server and empty the list. */
  async clear(): Promise<number> {
    this.abort?.abort();
    const removed = await this.api.clear();
    this.dispatch({ type: "cleared" });
    return removed;
  }

  /** Append the next older page. */
  async loadMore(): Promise<void> {
    const { next, loading, gen, filter } = this.log;
    if (next === null || loading) return;
    this.log = { ...this.log, loading: true };
    this.publish();
    try {
      const page = await this.api.list(this.opts.device, filter, { cursor: next });
      this.dispatch({ type: "older", gen, events: page.events, next: page.next });
    } catch (e) {
      this.dispatch({ type: "failed", gen, error: errorText(e) });
    }
  }

  private async query(filter: LogFilter | undefined): Promise<void> {
    this.abort?.abort();
    const abort = new AbortController();
    this.abort = abort;
    this.dispatch({ type: "query", filter });
    const { gen } = this.log;
    try {
      const page = await this.api.list(this.opts.device, this.log.filter, {
        signal: abort.signal,
      });
      this.dispatch({ type: "page", gen, events: page.events, next: page.next });
    } catch (e) {
      if (abort.signal.aborted) return;
      this.dispatch({ type: "failed", gen, error: errorText(e) });
    }
  }

  private setStatus(status: SocketStatus): void {
    if (this.status === status) return;
    this.status = status;
    this.publish();
  }

  private connect(): void {
    if (this.closed || isSessionLost()) return;
    this.setStatus(this.attempt === 0 ? "connecting" : "reconnecting");
    const url = eventsUrl(this.opts.device);
    let ws: WebSocket;
    try {
      ws = this.opts.createSocket ? this.opts.createSocket(url) : new WebSocket(url);
    } catch {
      this.scheduleReconnect();
      return;
    }
    this.ws = ws;
    ws.onopen = () => {
      // A reconnect may have missed events: read the newest page again (merged by id).
      if (this.attempt > 0) this.refresh();
      this.attempt = 0;
      this.setStatus("connected");
    };
    ws.onmessage = (ev: MessageEvent) => {
      if (typeof ev.data !== "string") return;
      const event = parseEventFrame(ev.data);
      if (event) this.dispatch({ type: "live", event });
    };
    ws.onclose = () => {
      if (this.ws !== ws) return;
      this.ws = null;
      checkSessionAfterClose();
      this.scheduleReconnect();
    };
    ws.onerror = () => undefined;
  }

  private scheduleReconnect(): void {
    if (this.closed || isSessionLost()) return;
    this.setStatus("reconnecting");
    const delay = backoffDelay(this.attempt);
    this.attempt += 1;
    this.reconnectTimer = setTimeout(() => this.connect(), delay);
  }
}
