// Pure helpers for the hover preview: where a click would land, how that is worded, and the
// short easing used for the VFO marker glide. The canvas drawing lives in interaction.ts.

import { snapFrequency } from "../vfo/model";
import { formatFrequency } from "./freq";
import type { Span } from "./view";

/** Duration of the marker glide after a click. */
export const GLIDE_MS = 150;
/** How long the click pulse takes to fade. */
export const PULSE_MS = 450;

/**
 * Frequency a plain click at `hz` would tune to: snapped to the step of the selected VFO, or
 * rounded to whole Hz when the click would retune the device centre.
 */
export function snapClick(hz: number, stepHz: number | null): number {
  return stepHz !== null && stepHz > 0 ? snapFrequency(hz, stepHz) : Math.round(hz);
}

export type PreviewTarget =
  | { kind: "vfo"; id: number }
  | { kind: "centre" }
  | { kind: "create" }
  | { kind: "grab"; id: number };

export function isOutside(window: Span | null, hz: number): boolean {
  return window !== null && (hz < window.startHz || hz > window.endHz);
}

/** Text of the floating chip: the frequency and what a click does there. */
export function previewLabel(hz: number, target: PreviewTarget, outside: boolean): string {
  const freq = `${formatFrequency(hz)} MHz`;
  switch (target.kind) {
    case "vfo":
      return `${freq}  → VFO ${target.id}${outside ? " (outside window)" : ""}`;
    case "centre":
      return `${freq}  → centre${outside ? " (outside window)" : ""}`;
    case "create":
      return `${freq}  → new VFO`;
    case "grab":
      return `${freq}  VFO ${target.id}`;
  }
}

/** Labels at the top edge of the shaded parts of the span (long form first, then short). */
export const OUTSIDE_LABEL = "outside tuning window";
export const OUTSIDE_LABEL_SHORT = "outside";

/** Explanation shown as tooltip while the pointer is over the shaded part of the span. */
export const OUTSIDE_WINDOW_HINT =
  "Outside the tuning window: the receiver's band edges are unusable. A VFO placed here is paused, or the daemon retunes the centre to follow it.";

/** Ease-out cubic of the linear progress `t` (clamped to 0..1). */
export function easeOut(t: number): number {
  const c = Math.min(1, Math.max(0, t));
  return 1 - (1 - c) ** 3;
}

/** Position of a glide from `from` to `to` after `elapsedMs`; `reduced` jumps straight to `to`. */
export function glide(from: number, to: number, elapsedMs: number, reduced: boolean): number {
  if (reduced || elapsedMs >= GLIDE_MS) return to;
  return from + (to - from) * easeOut(elapsedMs / GLIDE_MS);
}

/** True when the user asked the OS for less motion (false where matchMedia is unavailable). */
export function prefersReducedMotion(): boolean {
  return (
    typeof window !== "undefined" &&
    typeof window.matchMedia === "function" &&
    window.matchMedia("(prefers-reduced-motion: reduce)").matches
  );
}
