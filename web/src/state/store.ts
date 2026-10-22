// The single store for the confirmed device state. It is fed by the state socket
// (/api/v1/ws/state, docs/protocol-ws.md) and by state documents in control replies; the tuning
// and VFO controllers take their confirmed state from it (`bindState`). The socket reconnects
// with backoff and stays down while the session is lost, like the spectrum and audio sockets.

import { isSessionLost, verifySession } from "../auth/session";
import { backoffDelay, type SocketStatus } from "../spectrum/socket";
import { parseStateFrame, type StateDoc } from "./doc";

export interface StateSnapshot {
  /** Newest state document; null until the first frame (or REST fallback) arrived. */
  doc: StateDoc | null;
  status: SocketStatus;
}

export interface StateStoreOptions {
  device: string;
  /** Override for tests. */
  url?: string;
  createSocket?: (url: string) => WebSocket;
}

export function stateUrl(device: string, loc: Pick<Location, "protocol" | "host"> = location) {
  const scheme = loc.protocol === "https:" ? "wss:" : "ws:";
  return `${scheme}//${loc.host}/api/v1/ws/state?dev=${encodeURIComponent(device)}`;
}

export class StateStore {
  private ws: WebSocket | null = null;
  private attempt = 0;
  private reconnectTimer: ReturnType<typeof setTimeout> | undefined;
  private closed = true;
  private snapshot: StateSnapshot = { doc: null, status: "connecting" };
  private readonly listeners = new Set<() => void>();

  constructor(private readonly opts: StateStoreOptions) {}

  readonly subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  };

  readonly getSnapshot = (): StateSnapshot => this.snapshot;

  start(): void {
    if (!this.closed) return;
    this.closed = false;
    this.attempt = 0;
    this.connect();
  }

  stop(): void {
    this.closed = true;
    clearTimeout(this.reconnectTimer);
    const ws = this.ws;
    this.ws = null;
    if (ws) {
      ws.onopen = ws.onmessage = ws.onclose = ws.onerror = null;
      ws.close();
    }
  }

  /** Feed a state document from a REST reply (profile load, lock, ...). */
  ingest(doc: StateDoc): void {
    this.snapshot = { ...this.snapshot, doc };
    this.emit();
  }

  private setStatus(status: SocketStatus): void {
    if (this.snapshot.status === status) return;
    this.snapshot = { ...this.snapshot, status };
    this.emit();
  }

  private emit(): void {
    for (const l of [...this.listeners]) l();
  }

  private connect(): void {
    if (this.closed || isSessionLost()) return;
    this.setStatus(this.attempt === 0 ? "connecting" : "reconnecting");
    const url = this.opts.url ?? stateUrl(this.opts.device);
    let ws: WebSocket;
    try {
      ws = this.opts.createSocket ? this.opts.createSocket(url) : new WebSocket(url);
    } catch {
      this.scheduleReconnect();
      return;
    }
    this.ws = ws;
    let opened = false;
    ws.onopen = () => {
      opened = true;
      this.attempt = 0;
      this.setStatus("connected");
    };
    ws.onmessage = (ev: MessageEvent) => {
      if (typeof ev.data !== "string") return;
      const doc = parseStateFrame(ev.data);
      if (doc) this.ingest(doc);
    };
    ws.onclose = () => {
      if (this.ws !== ws) return;
      this.ws = null;
      // A rejected upgrade (401) looks like any failed connection: let the session check decide.
      if (!opened) void verifySession();
      this.scheduleReconnect();
    };
    // `close` always follows `error`.
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

/**
 * Forward every new document of `store` (and the current one) to the controllers.
 * Returns the unsubscribe function.
 */
export function bindState(
  store: StateStore,
  sinks: {
    tuning: { ingest(state: StateDoc["device"]): void };
    vfos: { ingest(vfos: StateDoc["vfos"]): void };
  },
): () => void {
  let last: StateDoc | null = null;
  const push = () => {
    const doc = store.getSnapshot().doc;
    if (!doc || doc === last) return;
    last = doc;
    sinks.tuning.ingest(doc.device);
    sinks.vfos.ingest(doc.vfos);
  };
  push();
  return store.subscribe(push);
}
