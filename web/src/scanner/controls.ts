// The scanner's commands as one external store: the panel buttons and the keyboard shortcuts both
// go through it, it remembers the last start request (so a shortcut can start the same scan
// again) and keeps the busy flag and the last error.

import { toasts } from "../ui";
import { createScannerApi, describeScanError, type ScanAction, type ScannerApi } from "./api";
import { isRunning, type ScannerState } from "./model";

export interface ControlsSnapshot {
  busy: boolean;
  error: string | null;
}

type Body = Record<string, unknown>;

const STORAGE_KEY = "superhet.scanner.last.v1";

function loadLast(): Body | null {
  try {
    const raw = window.localStorage.getItem(STORAGE_KEY);
    const parsed: unknown = raw === null ? null : JSON.parse(raw);
    return typeof parsed === "object" && parsed !== null && !Array.isArray(parsed)
      ? (parsed as Body)
      : null;
  } catch {
    return null;
  }
}

function saveLast(body: Body): void {
  try {
    // A channel list can be long; the shortcut restarts it, so keep it unless it is huge.
    const text = JSON.stringify(body);
    if (text.length < 200_000) window.localStorage.setItem(STORAGE_KEY, text);
  } catch {
    // Not persisted; the start still works.
  }
}

export class ScanControls {
  private snap: ControlsSnapshot = { busy: false, error: null };
  private state: ScannerState | null = null;
  private last: Body | null = loadLast();
  private readonly listeners = new Set<() => void>();

  constructor(
    private readonly device: string,
    private readonly getVfoId: () => number | null,
    private readonly api: ScannerApi = createScannerApi(),
  ) {}

  readonly subscribe = (fn: () => void): (() => void) => {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  };

  readonly getSnapshot = (): ControlsSnapshot => this.snap;

  /** The newest scanner state of the device (from the state feed). */
  ingest(state: ScannerState | null): void {
    this.state = state;
  }

  /** Whether a start request has been used before (a shortcut can repeat it). */
  hasLast(): boolean {
    return this.last !== null;
  }

  private set(next: Partial<ControlsSnapshot>): void {
    this.snap = { ...this.snap, ...next };
    for (const fn of this.listeners) fn();
  }

  private async run(action: ScanAction, body?: Body): Promise<boolean> {
    this.set({ busy: true, error: null });
    try {
      await this.api.send(this.device, action, body);
      this.set({ busy: false });
      return true;
    } catch (e) {
      const message = describeScanError(e);
      this.set({ busy: false, error: message });
      toasts.push({ key: "scanner-error", tone: "warn", title: "Scanner", body: message });
      return false;
    }
  }

  async start(body: Body): Promise<boolean> {
    const withVfo = { ...body };
    const vfo = this.getVfoId();
    if (vfo !== null) withVfo.vfo = vfo;
    saveLast(body);
    this.last = body;
    return this.run("start", withVfo);
  }

  stop = () => this.run("stop");
  pause = () => this.run("pause");
  resume = () => this.run("resume");
  skip = () => this.run("skip");

  /** Lock out a frequency; without one, the held channel. */
  lockout = (freqHz?: number) =>
    this.run("lockout", freqHz === undefined ? { v: 1 } : { v: 1, freq_hz: Math.round(freqHz) });

  /** Take a frequency (or, without one, all) off the lockout list. */
  unlock = (freqHz?: number) =>
    this.run(
      "unlock",
      freqHz === undefined ? { v: 1, all: true } : { v: 1, freq_hz: Math.round(freqHz) },
    );

  /**
   * Keyboard: stop a running scan, or start the last one again. Returns `setup` when there is
   * nothing to repeat yet (the caller opens the Scanner tab).
   */
  toggle(): "started" | "stopped" | "setup" {
    if (isRunning(this.state)) {
      void this.stop();
      return "stopped";
    }
    if (this.last === null) return "setup";
    void this.start(this.last);
    return "started";
  }

  /** Pause a running scan, or continue a paused one; true when something was sent. */
  togglePause(): boolean {
    if (this.state === null || !isRunning(this.state)) return false;
    void (this.state.state === "paused" ? this.resume() : this.pause());
    return true;
  }

  /** Skip the held channel or the current window; true when something was sent. */
  skipNow(): boolean {
    if (this.state === null || !isRunning(this.state)) return false;
    void this.skip();
    return true;
  }

  /** Lock out the held channel; true when there is one. */
  lockoutHeld(): boolean {
    if (this.state?.current == null || this.state.state === "idle") return false;
    void this.lockout();
    return true;
  }
}
