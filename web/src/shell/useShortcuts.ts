// Applies the keyboard shortcuts (see shortcuts.ts) to the receiver. One window listener for the
// app's lifetime; it reads the latest receiver state through a ref.

import { useEffect, useRef } from "react";
import type { AudioController } from "../audio/controller";
import { toasts } from "../ui";
import type { VfoController } from "../vfo/controller";
import type { AutoSquelch } from "../vfo/autoSquelch";
import { stepFor, stepFrequency, type StepChoice } from "../vfo/model";
import { ignoresShortcuts, shortcutFor, type ShortcutAction } from "./shortcuts";

export interface ShortcutContext {
  vfoCtl: Pick<VfoController, "getSnapshot" | "select" | "update" | "remove" | "create">;
  audio: Pick<AudioController, "getSnapshot" | "setListening" | "getRecentLevels">;
  autoSquelch: Pick<AutoSquelch, "toggle">;
  step: StepChoice;
  zoom: (factor: number) => void;
  resetZoom: () => void;
  /** Create a VFO at this frequency (the display centre for the N key). */
  createVfoAt: (hz: number | null) => void;
  centerHz: number | null;
  startBookmark: () => void;
  openHelp: () => void;
}

/** Run one shortcut; true when it did something (the caller then swallows the key event). */
export function runShortcut(action: ShortcutAction, ctx: ShortcutContext): boolean {
  const snap = ctx.vfoCtl.getSnapshot();
  const active = snap.vfos.find((v) => v.id === snap.selectedId) ?? null;
  switch (action.type) {
    case "step":
      if (!active) return false;
      ctx.vfoCtl.update(active.id, {
        freqHz: stepFrequency(active.freqHz, stepFor(ctx.step, active.mode), action.steps),
      });
      return true;
    case "vfo": {
      if (snap.vfos.length === 0) return false;
      const at = snap.vfos.findIndex((v) => v.id === snap.selectedId);
      const len = snap.vfos.length;
      const index = at < 0 ? (action.delta > 0 ? 0 : len - 1) : (at + action.delta + len) % len;
      const target = snap.vfos[index];
      if (target) ctx.vfoCtl.select(target.id);
      return true;
    }
    case "mode":
      if (!active) return false;
      if (action.mode === null) {
        toasts.push({
          key: "mode-unsupported",
          tone: "info",
          title: `${action.label} is not available yet`,
          body: "Only WFM, NFM and AM are implemented in the receiver so far.",
        });
        return true;
      }
      // A mode change resets the bandwidth to the new mode's default, as in the dock.
      if (action.mode !== active.mode) {
        ctx.vfoCtl.update(active.id, { mode: action.mode, bandwidthHz: null });
      }
      return true;
    case "listen":
      if (!active) return false;
      ctx.audio.setListening(
        active.id,
        !(ctx.audio.getSnapshot().levels[active.id]?.listening ?? true),
      );
      return true;
    case "autoSquelch":
      if (!active) return false;
      ctx.autoSquelch.toggle(active, ctx.audio, (id, patch) => ctx.vfoCtl.update(id, patch));
      return true;
    case "bookmark":
      ctx.startBookmark();
      return true;
    case "zoom":
      ctx.zoom(action.factor);
      return true;
    case "zoomReset":
      ctx.resetZoom();
      return true;
    case "newVfo":
      ctx.createVfoAt(ctx.centerHz);
      return true;
    case "delete": {
      if (!active) return false;
      const gone = active;
      ctx.vfoCtl.remove(gone.id);
      toasts.push({
        key: "vfo-removed",
        tone: "info",
        title: `VFO ${gone.id} removed`,
        action: {
          label: "Undo",
          onClick: () =>
            ctx.vfoCtl.create({
              freqHz: gone.freqHz,
              mode: gone.mode,
              bandwidthHz: gone.bandwidthHz,
              squelchDbfs: gone.squelchDbfs,
            }),
        },
      });
      return true;
    }
    case "help":
      ctx.openHelp();
      return true;
  }
}

/** Listen for the shortcuts on the window while mounted. */
export function useShortcuts(ctx: ShortcutContext): void {
  const ref = useRef(ctx);
  useEffect(() => {
    ref.current = ctx;
  });
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.defaultPrevented || e.isComposing) return;
      const action = shortcutFor(e);
      if (!action || ignoresShortcuts(e.target)) return;
      if (runShortcut(action, ref.current)) e.preventDefault();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);
}
