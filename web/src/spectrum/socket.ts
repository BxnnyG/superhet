// WebSocket client for /api/v1/ws/spectrum with automatic reconnect.
// Frames are delivered through a callback; no per-frame state is kept here.

import { isSessionLost, verifySession } from "../auth/session";
import { decodeFrame, type SpectrumFrame } from "./frame";

export type SocketStatus = "connecting" | "connected" | "reconnecting";

export const MIN_BINS = 16;
export const MAX_BINS = 4096;
const BACKOFF_MIN_MS = 250;
const BACKOFF_MAX_MS = 10_000;
const VIEW_DEBOUNCE_MS = 150;

export interface SpectrumView {
  startHz?: number;
  endHz?: number;
  maxFps?: number;
}

export interface SpectrumSocketOptions {
  device: string;
  /** Desired bin count (canvas device pixels); clamped to 16..4096. */
  getBins: () => number;
  onFrame: (frame: SpectrumFrame) => void;
  onStatus?: (status: SocketStatus) => void;
  onError?: (message: string) => void;
  /** Override for tests. */
  url?: string;
  createSocket?: (url: string) => WebSocket;
}

export function spectrumUrl(device: string, loc: Pick<Location, "protocol" | "host"> = location) {
  const scheme = loc.protocol === "https:" ? "wss:" : "ws:";
  return `${scheme}//${loc.host}/api/v1/ws/spectrum?dev=${encodeURIComponent(device)}`;
}

export function backoffDelay(attempt: number): number {
  return Math.min(BACKOFF_MAX_MS, BACKOFF_MIN_MS * 2 ** attempt);
}

export function clampBins(bins: number): number {
  if (!Number.isFinite(bins)) return MIN_BINS;
  return Math.min(MAX_BINS, Math.max(MIN_BINS, Math.round(bins)));
}

export class SpectrumSocket {
  private ws: WebSocket | null = null;
  private attempt = 0;
  private reconnectTimer: ReturnType<typeof setTimeout> | undefined;
  private viewTimer: ReturnType<typeof setTimeout> | undefined;
  private closed = true;
  private status: SocketStatus | null = null;
  private view: SpectrumView = {};

  constructor(private readonly opts: SpectrumSocketOptions) {}

  start(): void {
    if (!this.closed) return;
    this.closed = false;
    this.attempt = 0;
    this.connect();
  }

  stop(): void {
    this.closed = true;
    clearTimeout(this.reconnectTimer);
    clearTimeout(this.viewTimer);
    const ws = this.ws;
    this.ws = null;
    if (ws) {
      ws.onopen = ws.onmessage = ws.onclose = ws.onerror = null;
      ws.close();
    }
  }

  /** Change the requested span (undefined fields mean full frame / defaults) and resend. */
  setView(view: SpectrumView): void {
    this.view = view;
    this.sendView();
  }

  /** Call after the canvas size changed; the new view is sent debounced. */
  notifyResize(): void {
    clearTimeout(this.viewTimer);
    this.viewTimer = setTimeout(() => this.sendView(), VIEW_DEBOUNCE_MS);
  }

  private setStatus(status: SocketStatus): void {
    if (this.status === status) return;
    this.status = status;
    this.opts.onStatus?.(status);
  }

  private connect(): void {
    if (this.closed || isSessionLost()) return;
    this.setStatus(this.attempt === 0 ? "connecting" : "reconnecting");
    const url = this.opts.url ?? spectrumUrl(this.opts.device);
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
      this.setStatus("connected");
      this.sendView();
    };
    ws.onmessage = (ev: MessageEvent) => {
      if (typeof ev.data === "string") {
        this.handleText(ev.data);
        return;
      }
      if (!(ev.data instanceof ArrayBuffer)) return;
      try {
        this.opts.onFrame(decodeFrame(ev.data));
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

  private sendView(): void {
    const ws = this.ws;
    if (!ws || ws.readyState !== WebSocket.OPEN) return;
    const msg: Record<string, unknown> = {
      type: "view",
      v: 1,
      bins: clampBins(this.opts.getBins()),
    };
    const { startHz, endHz, maxFps } = this.view;
    if (startHz !== undefined && endHz !== undefined) {
      msg.start_hz = startHz;
      msg.end_hz = endHz;
    }
    if (maxFps !== undefined) msg.max_fps = maxFps;
    ws.send(JSON.stringify(msg));
  }
}
