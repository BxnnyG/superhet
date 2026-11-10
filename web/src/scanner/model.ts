// Scanner: the `scanner` object of the device state document and the start request (docs/scanner.md).
// Pure parsing and request building; the panel and the controller live next to this file.

import type { Bookmark } from "../bookmarks/model";
import { bandwidthOf, modeOf, type Plan } from "../channels/schema";
import type { VfoMode } from "../vfo/api";

export type ScanStateName = "idle" | "scanning" | "holding" | "paused" | "blocked";
export type ResumeMode = "hang" | "timeout" | "hold";

export interface ScanChannel {
  freqHz: number;
  mode: VfoMode;
  bandwidthHz: number | null;
  label: string | null;
  level: { dbfs: number; marginDb: number } | null;
  priority: boolean;
}

export interface ScanHit {
  tsMs: number;
  freqHz: number;
  mode: VfoMode | null;
  bandwidthHz: number | null;
  label: string | null;
  marginDb: number | null;
  priority: boolean;
}

export interface ScanConfig {
  kind: "range" | "list";
  name: string | null;
  count: number;
  thresholdDb: number;
  thresholdDbfs: number | null;
  dwellMs: number;
  hangMs: number;
  timeoutMs: number;
  resume: ResumeMode;
  priorityHz: number | null;
  priorityIntervalMs: number;
}

export interface ScanWindow {
  index: number;
  count: number;
  lowHz: number;
  highHz: number;
}

export interface ScannerState {
  state: ScanStateName;
  /** While holding: the signal is there (`signal`) or gone and the hang time runs (`hang`). */
  phase: "signal" | "hang" | null;
  vfo: number | null;
  pass: number;
  window: ScanWindow | null;
  /** The held channel. */
  current: ScanChannel | null;
  /** Why nothing is scanned (`nothing_to_scan`, `locked`). */
  blocked: string | null;
  /** The tuning lock keeps the scan inside its current window. */
  restricted: boolean;
  /** Why the last scan ended (`stopped`, `claimed`, `vfo_removed`, ...). */
  stopped: string | null;
  startedMs: number | null;
  config: ScanConfig | null;
  lockouts: number[];
  hits: ScanHit[];
}

type Rec = Record<string, unknown>;
const isRec = (v: unknown): v is Rec => typeof v === "object" && v !== null && !Array.isArray(v);
const num = (v: unknown): number | null => (typeof v === "number" && Number.isFinite(v) ? v : null);
const str = (v: unknown): string | null => (typeof v === "string" ? v : null);
const MODES = new Set(["wfm", "nfm", "am", "usb", "lsb", "cw"]);
const mode = (v: unknown): VfoMode | null =>
  typeof v === "string" && MODES.has(v) ? (v as VfoMode) : null;
const STATES = new Set<string>(["idle", "scanning", "holding", "paused", "blocked"]);

function parseChannel(raw: unknown): ScanChannel | null {
  if (!isRec(raw)) return null;
  const freqHz = num(raw.freq_hz);
  const m = mode(raw.mode);
  if (freqHz === null || m === null) return null;
  const dbfs = num(raw.level_dbfs);
  const marginDb = num(raw.margin_db);
  return {
    freqHz,
    mode: m,
    bandwidthHz: num(raw.bandwidth_hz),
    label: str(raw.label),
    level: dbfs !== null && marginDb !== null ? { dbfs, marginDb } : null,
    priority: raw.priority === true,
  };
}

function parseHit(raw: unknown): ScanHit | null {
  if (!isRec(raw)) return null;
  const freqHz = num(raw.freq_hz);
  const tsMs = num(raw.ts_ms);
  if (freqHz === null || tsMs === null) return null;
  return {
    tsMs,
    freqHz,
    mode: mode(raw.mode),
    bandwidthHz: num(raw.bandwidth_hz),
    label: str(raw.label),
    marginDb: num(raw.margin_db),
    priority: raw.priority === true,
  };
}

function parseConfig(raw: unknown): ScanConfig | null {
  if (!isRec(raw)) return null;
  const resume = raw.resume;
  return {
    kind: raw.kind === "range" ? "range" : "list",
    name: str(raw.name),
    count: num(raw.count) ?? 0,
    thresholdDb: num(raw.threshold_db) ?? 10,
    thresholdDbfs: num(raw.threshold_dbfs),
    dwellMs: num(raw.dwell_ms) ?? 150,
    hangMs: num(raw.hang_ms) ?? 2000,
    timeoutMs: num(raw.timeout_ms) ?? 10000,
    resume: resume === "timeout" || resume === "hold" ? resume : "hang",
    priorityHz: num(raw.priority_hz),
    priorityIntervalMs: num(raw.priority_interval_ms) ?? 5000,
  };
}

/** The `scanner` object of a state document; null when it is absent or unreadable. */
export function parseScannerState(raw: unknown): ScannerState | null {
  if (!isRec(raw) || typeof raw.state !== "string" || !STATES.has(raw.state)) return null;
  const w = raw.window;
  const window: ScanWindow | null =
    isRec(w) && num(w.index) !== null && num(w.count) !== null
      ? {
          index: num(w.index) ?? 1,
          count: num(w.count) ?? 1,
          lowHz: num(w.low_hz) ?? 0,
          highHz: num(w.high_hz) ?? 0,
        }
      : null;
  return {
    state: raw.state as ScanStateName,
    phase: raw.phase === "signal" || raw.phase === "hang" ? raw.phase : null,
    vfo: num(raw.vfo),
    pass: num(raw.pass) ?? 0,
    window,
    current: parseChannel(raw.current),
    blocked: str(raw.blocked),
    restricted: raw.restricted !== undefined,
    stopped: str(raw.stopped),
    startedMs: num(raw.started_ms),
    config: parseConfig(raw.config),
    lockouts: Array.isArray(raw.lockouts)
      ? raw.lockouts.filter((x): x is number => typeof x === "number")
      : [],
    hits: Array.isArray(raw.hits)
      ? raw.hits.map(parseHit).filter((h): h is ScanHit => h !== null)
      : [],
  };
}

export function isRunning(s: ScannerState | null): boolean {
  return s !== null && s.state !== "idle";
}

/** Short status text and tone for the badge. */
export function statusLabel(s: ScannerState | null): {
  text: string;
  tone: "neutral" | "accent" | "success" | "warn";
} {
  if (s === null || s.state === "idle") return { text: "Idle", tone: "neutral" };
  switch (s.state) {
    case "scanning":
      return { text: s.restricted ? "Scanning (tuning locked)" : "Scanning", tone: "accent" };
    case "holding":
      return s.phase === "hang"
        ? { text: "Hang time", tone: "warn" }
        : { text: "Signal", tone: "success" };
    case "paused":
      return { text: "Paused", tone: "warn" };
    case "blocked":
      return {
        text: s.blocked === "locked" ? "Blocked by the tuning lock" : "Nothing to scan",
        tone: "warn",
      };
  }
}

/** Why a finished scan ended, for the idle state. */
export function stoppedText(reason: string | null): string | null {
  switch (reason) {
    case "claimed":
      return "Stopped: a plugin took the device.";
    case "vfo_removed":
      return "Stopped: the VFO was removed.";
    case "restarted":
    case "stopped":
    case null:
      return null;
    default:
      return `Stopped (${reason}).`;
  }
}

// --- Start request -------------------------------------------------------------------------

export interface ScanSettings {
  thresholdDb: number;
  /** null = relative to the noise floor. */
  thresholdDbfs: number | null;
  dwellMs: number;
  hangMs: number;
  timeoutMs: number;
  resume: ResumeMode;
  priorityHz: number | null;
  priorityIntervalMs: number;
}

export const DEFAULT_SETTINGS: ScanSettings = {
  thresholdDb: 10,
  thresholdDbfs: null,
  dwellMs: 150,
  hangMs: 2000,
  timeoutMs: 10000,
  resume: "hang",
  priorityHz: null,
  priorityIntervalMs: 5000,
};

export type ScanSource =
  | {
      kind: "range";
      startHz: number;
      stopHz: number;
      stepHz: number;
      mode: VfoMode | null;
    }
  | { kind: "list"; name: string; channels: ScanListChannel[] };

export interface ScanListChannel {
  freqHz: number;
  mode: VfoMode | null;
  bandwidthHz: number | null;
  label: string | null;
}

/** Most channels the daemon accepts in a list. */
export const MAX_LIST_CHANNELS = 2000;
/** Most channels a range may produce. */
export const MAX_RANGE_CHANNELS = 20000;

export function rangeCount(startHz: number, stopHz: number, stepHz: number): number {
  if (!(stepHz > 0) || stopHz < startHz) return 0;
  return Math.floor((stopHz - startHz) / stepHz) + 1;
}

/** Why a source cannot be started, or null. */
export function sourceProblem(source: ScanSource): string | null {
  if (source.kind === "range") {
    if (!(source.startHz > 0) || !(source.stopHz > 0)) return "Enter a start and a stop frequency.";
    if (source.stopHz < source.startHz) return "The stop frequency is below the start.";
    if (source.stepHz < 100) return "The step must be at least 100 Hz.";
    const n = rangeCount(source.startHz, source.stopHz, source.stepHz);
    if (n > MAX_RANGE_CHANNELS) {
      return `${n.toLocaleString("en")} channels is too many (at most ${MAX_RANGE_CHANNELS.toLocaleString("en")}). Use a larger step.`;
    }
    return null;
  }
  if (source.channels.length === 0) return "The list has no channels.";
  if (source.channels.length > MAX_LIST_CHANNELS) {
    return `The list has ${source.channels.length} channels; at most ${MAX_LIST_CHANNELS}.`;
  }
  return null;
}

/** The body of `POST /api/v1/devices/{dev}/scanner/start`. */
export function startBody(
  source: ScanSource,
  settings: ScanSettings,
  vfo: number | null,
  lockouts: readonly number[] = [],
): Rec {
  const body: Rec = {
    v: 1,
    threshold_db: settings.thresholdDb,
    dwell_ms: settings.dwellMs,
    hang_ms: settings.hangMs,
    resume: settings.resume,
    timeout_ms: settings.timeoutMs,
  };
  if (settings.thresholdDbfs !== null) body.threshold_dbfs = settings.thresholdDbfs;
  if (vfo !== null) body.vfo = vfo;
  if (lockouts.length > 0) body.lockout_hz = lockouts;
  if (settings.priorityHz !== null) {
    body.priority = {
      freq_hz: Math.round(settings.priorityHz),
      interval_ms: settings.priorityIntervalMs,
    };
  }
  if (source.kind === "range") {
    const range: Rec = {
      kind: "range",
      start_hz: Math.round(source.startHz),
      stop_hz: Math.round(source.stopHz),
      step_hz: Math.round(source.stepHz),
    };
    if (source.mode !== null) range.mode = source.mode;
    body.source = range;
  } else {
    body.source = {
      kind: "list",
      name: source.name,
      channels: source.channels.map((c) => {
        const ch: Rec = { freq_hz: Math.round(c.freqHz) };
        if (c.mode !== null) ch.mode = c.mode;
        if (c.bandwidthHz !== null) ch.bandwidth_hz = c.bandwidthHz;
        if (c.label !== null && c.label !== "") ch.label = c.label.slice(0, 64);
        return ch;
      }),
    };
  }
  return body;
}

// --- Channel lists -------------------------------------------------------------------------

/** Bookmarks as scan channels: a group, or every bookmark flagged for scanning. */
export function bookmarkChannels(
  bookmarks: readonly Bookmark[],
  groupId: string | "flagged" | "all",
): ScanListChannel[] {
  const chosen = bookmarks.filter((b) =>
    groupId === "all" ? true : groupId === "flagged" ? b.scan.include : b.groupId === groupId,
  );
  return chosen.map((b) => ({
    freqHz: b.freqHz,
    mode: b.mode,
    bandwidthHz: b.bandwidthHz,
    label: b.name,
  }));
}

/** The channels of a channel plan (its channel list; rasters are scanned as a range). */
export function planChannels(plan: Plan): ScanListChannel[] {
  return plan.channels
    .map((c): ScanListChannel | null => {
      const m = modeOf(plan, c);
      return m === "dab"
        ? null
        : {
            freqHz: c.freqHz,
            mode: m,
            bandwidthHz: bandwidthOf(plan, c),
            label: `${plan.chip} ${c.number}`.trim(),
          };
    })
    .filter((c): c is ScanListChannel => c !== null);
}

/** Steps offered for a range scan, Hz. */
export const RANGE_STEPS_HZ: readonly number[] = [
  1000, 5000, 6250, 8330, 10000, 12500, 20000, 25000, 50000, 100000, 200000,
];
