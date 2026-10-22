// Squelch "Auto" state shared by the dock button and the S shortcut. Auto sets the threshold just
// above the noise floor seen in the last few seconds. It stays marked "auto" while the VFO still
// has exactly the applied value; toggling it again clears the squelch.

import type { AudioController } from "../audio/controller";
import { toasts } from "../ui";
import type { Vfo, VfoPatch } from "./api";
import { AUTO_SQUELCH_WINDOW_MS, autoSquelchDbfs } from "./model";

export class AutoSquelch {
  private applied: ReadonlyMap<number, number> = new Map();
  private readonly listeners = new Set<() => void>();

  readonly subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  };

  /** The value Auto applied per VFO id (replaced on every change). */
  readonly getSnapshot = (): ReadonlyMap<number, number> => this.applied;

  isAuto(vfo: Vfo): boolean {
    return vfo.squelchDbfs !== null && this.applied.get(vfo.id) === vfo.squelchDbfs;
  }

  /** The squelch was set by hand: Auto no longer applies. */
  forget(id: number): void {
    if (!this.applied.has(id)) return;
    const next = new Map(this.applied);
    next.delete(id);
    this.set(next);
  }

  /** Switch Auto on (measure the noise floor) or, when it is on, the squelch off. */
  toggle(
    vfo: Vfo,
    audio: Pick<AudioController, "getRecentLevels">,
    update: (id: number, patch: VfoPatch) => void,
  ): void {
    if (this.isAuto(vfo)) {
      this.forget(vfo.id);
      update(vfo.id, { squelchDbfs: null });
      return;
    }
    const value = autoSquelchDbfs(audio.getRecentLevels(vfo.id, AUTO_SQUELCH_WINDOW_MS));
    if (value === null) {
      toasts.push({
        key: "auto-squelch",
        tone: "info",
        title: "Not enough signal data yet",
        body: "Auto squelch needs a few seconds of channel level. Try again shortly.",
      });
      return;
    }
    this.set(new Map(this.applied).set(vfo.id, value));
    update(vfo.id, { squelchDbfs: value });
  }

  private set(next: ReadonlyMap<number, number>): void {
    this.applied = next;
    for (const l of [...this.listeners]) l();
  }
}
