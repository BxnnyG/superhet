// WebSocket client for /api/v1/ws/audio (docs/protocol-audio.md) with automatic reconnect.
// The subscription (set of VFO ids) is re-sent after every reconnect.

import { isSessionLost, verifySession } from "../auth/session";
import { decodeAudioFrame, type AudioFrame } from "./frame";
import { parseRds, type RdsState } from "./rds";
import { backoffDelay, type SocketStatus } from "../spectrum/socket";

/** The server accepts at most 8 distinct VFO ids per socket. */
export const MAX_SUBSCRIPTIONS = 8;

export interface AudioSocketOptions {
  device: string;
  onFrame: (frame: AudioFrame) => void;
  /** Complete RDS state of a VFO (an empty state clears the display). */
  onRds?: (vfo: number, state: RdsState) => void;
  onStatus?: (status: SocketStatus) => void;
  onError?: (message: string) => void;
  /** Override for tests. */
  url?: string;
  createSocket?: (url: string) => WebSocket;
}

export function audioUrl(device: string, loc: Pick<Location, "protocol" | "host"> = location) {
  const scheme = loc.protocol === "https:" ? "wss:" : "ws:";
  return `${scheme}//${loc.host}/api/v1/ws/audio?dev=${encodeURIComponent(device)}`;
}

/** Normalise a VFO id list: integers >= 1, unique, sorted, at most 8. */
export function normalizeVfoIds(ids: Iterable<number>): number[] {
  const set = new Set<number>();
  for (const id of ids) if (Number.isInteger(id) && id >= 1) set.add(id);
  return [...set].sort((a, b) => a - b).slice(0, MAX_SUBSCRIPTIONS);
}

export function subscribeMessage(ids: Iterable<number>): string {
  return JSON.stringify({ type: "subscribe", v: 1, vfos: normalizeVfoIds(ids) });
}

export class AudioSocket {
  private ws: WebSocket | null = null;
  private attempt = 0;
  private reconnectTimer: ReturnType<typeof setTimeout> | undefined;
  private closed = true;
  private status: SocketStatus | null = null;
  private ids: number[] = [];
  private sentKey: string | null = null;

  constructor(private readonly opts: AudioSocketOptions) {}

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

  /** Replace the subscribed VFO set; sent immediately when connected, else on (re)connect. */
  setVfos(ids: Iterable<number>): void {
    this.ids = normalizeVfoIds(ids);
    this.sendSubscription();
  }

  private setStatus(status: SocketStatus): void {
    if (this.status === status) return;
    this.status = status;
    this.opts.onStatus?.(status);
  }

  private connect(): void {
    if (this.closed || isSessionLost()) return;
    this.setStatus(this.attempt === 0 ? "connecting" : "reconnecting");
    const url = this.opts.url ?? audioUrl(this.opts.device);
    let ws: WebSocket;
    try {
      ws = this.opts.createSocket ? this.opts.createSocket(url) : new WebSocket(url);
    } catch {
      this.scheduleReconnect();
      return;
    }
    ws.binaryType = "arraybuffer";
    this.ws = ws;
    let opened = false;
    ws.onopen = () => {
      opened = true;
      this.attempt = 0;
      this.sentKey = null;
      this.setStatus("connected");
      this.sendSubscription();
    };
    ws.onmessage = (ev: MessageEvent) => {
      if (typeof ev.data === "string") {
        this.handleText(ev.data);
        return;
      }
      if (!(ev.data instanceof ArrayBuffer)) return;
      try {
        this.opts.onFrame(decodeAudioFrame(ev.data));
      } catch (e) {
        this.opts.onError?.(e instanceof Error ? e.message : String(e));
      }
    };
    ws.onclose = () => {
      if (this.ws !== ws) return;
      this.ws = null;
      // A rejected upgrade (401) looks like any failed connection: ask the server whether the
      // session is still valid; if not, the session is marked lost and reconnecting stops.
      if (!opened) void verifySession();
      this.scheduleReconnect();
    };
    // `close` always follows `error`; nothing to do here.
    ws.onerror = () => undefined;
  }

  private handleText(text: string): void {
    try {
      const msg: unknown = JSON.parse(text);
      const rds = parseRds(msg);
      if (rds) {
        this.opts.onRds?.(rds.vfo, rds.state);
        return;
      }
      if (
        typeof msg === "object" &&
        msg !== null &&
        "type" in msg &&
        msg.type === "error" &&
        "error" in msg
      ) {
        this.opts.onError?.(String(msg.error));
      }
    } catch {
      // Ignore malformed control messages.
    }
  }

  private scheduleReconnect(): void {
    if (this.closed || isSessionLost()) return;
    this.setStatus("reconnecting");
    const delay = backoffDelay(this.attempt);
    this.attempt += 1;
    this.reconnectTimer = setTimeout(() => this.connect(), delay);
  }

  private sendSubscription(): void {
    const ws = this.ws;
    if (!ws || ws.readyState !== WebSocket.OPEN) return;
    const key = this.ids.join(",");
    if (key === this.sentKey) return;
    this.sentKey = key;
    ws.send(subscribeMessage(this.ids));
  }
}
