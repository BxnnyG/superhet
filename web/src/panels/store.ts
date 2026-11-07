// The plugin panels the server offers, as an external store. Loaded once at start and then every
// half minute while the tab is visible (an admin installing a plugin makes a tab appear without a
// reload). A failure keeps what is known: panels are a convenience, never a reason to nag.

import { createPanelApi, type PanelApi, type PanelInfo } from "./api";

export interface PanelSnapshot {
  panels: readonly PanelInfo[];
  loaded: boolean;
}

export const PANEL_POLL_MS = 30_000;

const EMPTY: PanelSnapshot = { panels: [], loaded: false };

export class PanelStore {
  private snap: PanelSnapshot = EMPTY;
  private readonly listeners = new Set<() => void>();
  private timer: ReturnType<typeof setInterval> | undefined;
  private abort: AbortController | null = null;

  constructor(private readonly api: PanelApi = createPanelApi()) {}

  readonly subscribe = (fn: () => void): (() => void) => {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  };

  readonly getSnapshot = (): PanelSnapshot => this.snap;

  start(): void {
    if (this.abort) return;
    this.abort = new AbortController();
    void this.refresh();
    this.timer = setInterval(() => {
      if (typeof document === "undefined" || !document.hidden) void this.refresh();
    }, PANEL_POLL_MS);
  }

  stop(): void {
    clearInterval(this.timer);
    this.abort?.abort();
    this.abort = null;
  }

  async refresh(): Promise<void> {
    const signal = this.abort?.signal;
    try {
      const panels = await this.api.list(signal);
      if (signal?.aborted) return;
      const same = this.snap.loaded && JSON.stringify(panels) === JSON.stringify(this.snap.panels);
      if (same) return;
      this.snap = { panels, loaded: true };
    } catch {
      if (signal?.aborted) return;
      if (this.snap.loaded) return;
      this.snap = { panels: [], loaded: true };
    }
    for (const fn of [...this.listeners]) fn();
  }
}
