// What the Scanner tab remembers between visits: the source choice and the scan settings.

import type { VfoMode } from "../vfo/api";
import { DEFAULT_SETTINGS, type ResumeMode, type ScanSettings } from "./model";

export type SourceKind = "range" | "bookmarks" | "plan";

export interface ScannerPrefs {
  source: SourceKind;
  startMhz: number;
  stopMhz: number;
  stepHz: number;
  /** null = by band (the daemon picks the mode of each frequency). */
  mode: VfoMode | null;
  /** A bookmark group id, `flagged` (bookmarks marked for scanning) or `all`. */
  bookmarkGroup: string;
  planId: string | null;
  /** Record every hit as an audio recording while the scan runs (operators). */
  recordHits: boolean;
  settings: ScanSettings;
}

export const DEFAULT_PREFS: ScannerPrefs = {
  source: "range",
  startMhz: 144,
  stopMhz: 146,
  stepHz: 12500,
  mode: null,
  bookmarkGroup: "flagged",
  planId: null,
  recordHits: false,
  settings: DEFAULT_SETTINGS,
};

const KEY = "superhet.scanner.prefs.v1";

const num = (v: unknown, fallback: number, min: number, max: number): number =>
  typeof v === "number" && Number.isFinite(v) ? Math.min(max, Math.max(min, v)) : fallback;
const MODES = ["wfm", "nfm", "am", "usb", "lsb", "cw"];
const RESUME: ResumeMode[] = ["hang", "timeout", "hold"];

export function parsePrefs(raw: unknown): ScannerPrefs {
  if (typeof raw !== "object" || raw === null) return DEFAULT_PREFS;
  const r = raw as Record<string, unknown>;
  const s = (typeof r.settings === "object" && r.settings !== null ? r.settings : {}) as Record<
    string,
    unknown
  >;
  const d = DEFAULT_SETTINGS;
  return {
    source: r.source === "bookmarks" || r.source === "plan" ? r.source : "range",
    startMhz: num(r.startMhz, DEFAULT_PREFS.startMhz, 0.001, 10_000),
    stopMhz: num(r.stopMhz, DEFAULT_PREFS.stopMhz, 0.001, 10_000),
    stepHz: num(r.stepHz, DEFAULT_PREFS.stepHz, 100, 100_000_000),
    mode: typeof r.mode === "string" && MODES.includes(r.mode) ? (r.mode as VfoMode) : null,
    bookmarkGroup: typeof r.bookmarkGroup === "string" ? r.bookmarkGroup : "flagged",
    planId: typeof r.planId === "string" ? r.planId : null,
    recordHits: r.recordHits === true,
    settings: {
      thresholdDb: num(s.thresholdDb, d.thresholdDb, 1, 60),
      thresholdDbfs:
        typeof s.thresholdDbfs === "number" ? num(s.thresholdDbfs, -60, -140, 0) : null,
      dwellMs: num(s.dwellMs, d.dwellMs, 20, 5000),
      hangMs: num(s.hangMs, d.hangMs, 0, 60000),
      timeoutMs: num(s.timeoutMs, d.timeoutMs, 500, 600000),
      resume: RESUME.includes(s.resume as ResumeMode) ? (s.resume as ResumeMode) : "hang",
      priorityHz: typeof s.priorityHz === "number" ? num(s.priorityHz, 0, 1, 1e12) : null,
      priorityIntervalMs: num(s.priorityIntervalMs, d.priorityIntervalMs, 500, 600000),
    },
  };
}

export function loadPrefs(): ScannerPrefs {
  try {
    const raw = window.localStorage.getItem(KEY);
    return raw === null ? DEFAULT_PREFS : parsePrefs(JSON.parse(raw));
  } catch {
    return DEFAULT_PREFS;
  }
}

export function savePrefs(prefs: ScannerPrefs): void {
  try {
    window.localStorage.setItem(KEY, JSON.stringify(prefs));
  } catch {
    // Not persisted; the choice still applies for this session.
  }
}
