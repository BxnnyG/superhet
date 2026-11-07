// The postMessage bridge between the web app and a plugin panel (ADR-0022,
// docs/plugins-ui-panels.md). Version 1.
//
// The panel runs in an iframe with `sandbox="allow-scripts"` (opaque origin, no cookies, no API
// access: the page's CSP has `connect-src 'none'`). The web app does the reading on its own
// session and hands the panel a filtered copy; the panel can ask for exactly one thing, a tune
// request, which the app grants only for operators/admins and only when the manifest asks for it.
// Everything in this file is pure so the validation can be tested without a browser.

import type { DecoderEvent } from "../decoders/events";
import type { PluginInstance } from "../plugins/api";

export const BRIDGE_VERSION = 1;
/** Newest events handed to a panel. */
export const MAX_PANEL_EVENTS = 50;
/** Highest frequency a panel may ask for (Hz); the VFO controller clamps to the device anyway. */
export const MAX_TUNE_HZ = 6_000_000_000;
/** A panel may tune at most once per interval. */
export const MIN_TUNE_INTERVAL_MS = 1000;

/** What a panel may send. Anything else is dropped without an answer. */
export type GuestMessage = { type: "hello" } | { type: "tune"; freqHz: number };

function isPlain(v: unknown): v is Record<string, unknown> {
  if (typeof v !== "object" || v === null || Array.isArray(v)) return false;
  const proto: unknown = Object.getPrototypeOf(v);
  return proto === Object.prototype || proto === null;
}

/**
 * Validate a message from the panel: a plain object with exactly the fields of its type,
 * `superhet: 1`, a known `type` and, for `tune`, a safe integer frequency in 1..MAX_TUNE_HZ.
 * Unknown fields reject the message (the contract is closed; a typo must not look like success).
 */
export function parseGuestMessage(data: unknown): GuestMessage | null {
  if (!isPlain(data) || data.superhet !== BRIDGE_VERSION) return null;
  const keys = Object.keys(data);
  if (data.type === "hello") {
    return keys.length === 2 ? { type: "hello" } : null;
  }
  if (data.type === "tune") {
    const f = data.freq_hz;
    if (keys.length !== 3 || typeof f !== "number" || !Number.isSafeInteger(f)) return null;
    return f >= 1 && f <= MAX_TUNE_HZ ? { type: "tune", freqHz: f } : null;
  }
  return null;
}

/** Whether a message may be accepted from this sender: the frame's window, opaque origin. */
export function fromPanelFrame(
  event: Pick<MessageEvent, "source" | "origin">,
  frame: Window | null | undefined,
): boolean {
  return frame != null && event.source === frame && event.origin === "null";
}

/** Tune requests are honoured for operators and admins, and only for panels that ask for it. */
export function mayTune(manifestTune: boolean, isOperator: boolean): boolean {
  return manifestTune && isOperator;
}

/** At most one tune request per interval. */
export class TuneLimiter {
  private last = Number.NEGATIVE_INFINITY;
  constructor(
    private readonly now: () => number = () => Date.now(),
    private readonly intervalMs = MIN_TUNE_INTERVAL_MS,
  ) {}

  allow(): boolean {
    const t = this.now();
    if (t - this.last < this.intervalMs) return false;
    this.last = t;
    return true;
  }
}

// --- Host to panel ---------------------------------------------------------------------------

/** CSS variables of the app that panels receive as `--sh-<name>`. */
export const THEME_TOKENS = [
  "surface-0",
  "surface-1",
  "surface-2",
  "surface-3",
  "border",
  "border-strong",
  "fg",
  "fg-muted",
  "fg-subtle",
  "accent",
  "accent-hover",
  "accent-fg",
  "accent-soft",
  "success",
  "warn",
  "danger",
] as const;

const SAFE_VALUE = /^[#a-zA-Z0-9(),.%/ -]{1,80}$/;

export type ThemeTokens = Record<string, string>;

/** Current colours of the app, read from the computed style; unusual values are dropped. */
export function readTheme(root: HTMLElement = document.documentElement): ThemeTokens {
  const style = getComputedStyle(root);
  const out: ThemeTokens = {};
  for (const name of THEME_TOKENS) {
    const value = style.getPropertyValue(`--${name}`).trim();
    if (SAFE_VALUE.test(value)) out[name] = value;
  }
  const scheme = style.colorScheme.includes("dark") ? "dark" : "light";
  out.scheme = scheme;
  return out;
}

export interface PanelInstance {
  id: string;
  plugin_id: string;
  device: string | null;
  vfo: number | null;
  freq_hz: number | null;
  state: string;
  updated_ms: number;
}

/**
 * Instances of the plugins the panel may read, without configuration, process ids, error text or
 * counters (the same whitelist idea as the listener view of the API).
 */
export function panelInstances(
  instances: readonly PluginInstance[],
  reads: readonly string[],
): PanelInstance[] {
  return instances
    .filter((i) => reads.includes(i.pluginId) && i.state !== "stopped")
    .map((i) => ({
      id: i.id,
      plugin_id: i.pluginId,
      device: i.device,
      vfo: i.vfo,
      freq_hz: i.freqHz,
      state: i.state,
      updated_ms: i.updatedMs,
    }));
}

export interface PanelEvent {
  id: number;
  ts_ms: number;
  source: string;
  kind: string;
  value: string;
  device: string;
  vfo: number | null;
  freq_hz: number | null;
  meta: Record<string, unknown>;
}

/** The newest events of the plugins the panel may read (the list is newest first). */
export function panelEvents(
  events: readonly DecoderEvent[],
  reads: readonly string[],
): PanelEvent[] {
  const out: PanelEvent[] = [];
  for (const e of events) {
    if (!reads.includes(e.source)) continue;
    out.push({
      id: e.id,
      ts_ms: e.tsMs,
      source: e.source,
      kind: e.kind,
      value: e.value,
      device: e.device,
      vfo: e.vfo,
      freq_hz: e.freqHz,
      meta: e.meta,
    });
    if (out.length >= MAX_PANEL_EVENTS) break;
  }
  return out;
}

const msg = <T extends object>(type: string, body: T) => ({
  superhet: BRIDGE_VERSION,
  type,
  ...body,
});

export const initMessage = (pluginId: string, canTune: boolean, theme: ThemeTokens) =>
  msg("init", { plugin_id: pluginId, can_tune: canTune, theme });
export const themeMessage = (theme: ThemeTokens) => msg("theme", { theme });
export const instancesMessage = (instances: PanelInstance[]) => msg("instances", { instances });
export const eventsMessage = (events: PanelEvent[]) => msg("events", { events });
export const tuneResultMessage = (ok: boolean, error?: string) =>
  msg("tune_result", ok ? { ok } : { ok, error: error ?? "refused" });
