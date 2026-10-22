// Pure helpers for the spectrum gestures: click/drag threshold, passband edge dragging and the
// routing of the mouse wheel between zooming and tuning. The pointer plumbing lives in
// interaction.ts; keeping the rules here makes them testable without a canvas.

import type { VfoMode } from "../vfo/api";
import { bandwidthLimits, type Marker } from "../vfo/model";

/** Distance in CSS pixels below which a press counts as a click, not a drag. */
export const CLICK_SLOP_PX = 4;
/** Grab tolerance around a VFO centre line or passband edge (CSS px). */
export const VFO_GRAB_PX = 5;
/** Passband edges snap to multiples of this while dragging. */
export const BANDWIDTH_SNAP_HZ = 100;
/** Accumulated wheel pixels per tuning step (a mouse notch is ~100, a trackpad sends many small). */
export const WHEEL_STEP_PX = 80;

export function exceedsSlop(dxPx: number, slopPx = CLICK_SLOP_PX): boolean {
  return Math.abs(dxPx) > slopPx;
}

export type PassbandEdge = "lo" | "hi";

/**
 * Which edge of the (selected) VFO passband lies under `hz`, if any. Narrow passbands (edges
 * closer than the grab zone to the centre line) and the centre line itself keep the centre grab.
 */
export function hitPassbandEdge(m: Marker, hz: number, slopHz: number): PassbandEdge | null {
  const half = m.bandwidthHz / 2;
  if (half <= slopHz * 1.5 || Math.abs(hz - m.freqHz) <= slopHz) return null;
  const lo = Math.abs(hz - (m.freqHz - half));
  const hi = Math.abs(hz - (m.freqHz + half));
  if (Math.min(lo, hi) > slopHz) return null;
  return lo <= hi ? "lo" : "hi";
}

/** True when `hz` is inside the passband of `m` (or within `slopHz` of its centre line). */
export function isOverPassband(m: Marker, hz: number, slopHz: number): boolean {
  return Math.abs(hz - m.freqHz) <= Math.max(m.bandwidthHz / 2, slopHz);
}

/** Which side of the carrier a mode occupies: AM/FM both, USB upper, LSB lower. */
export type Sideband = "both" | "upper" | "lower";

/**
 * Bandwidth after dragging an edge to `pointerHz`: twice the distance to the carrier for
 * two-sided modes (the other edge mirrors), the distance for single-sideband modes. Snapped to
 * 100 Hz and clamped to the limits of the bandwidth presets of the mode.
 */
export function bandwidthFromEdge(
  mode: VfoMode,
  pointerHz: number,
  freqHz: number,
  sideband: Sideband = "both",
): number {
  const offset =
    sideband === "upper"
      ? pointerHz - freqHz
      : sideband === "lower"
        ? freqHz - pointerHz
        : 2 * Math.abs(pointerHz - freqHz);
  const { min, max } = bandwidthLimits(mode);
  const snapped = Math.round(Math.max(0, offset) / BANDWIDTH_SNAP_HZ) * BANDWIDTH_SNAP_HZ;
  return Math.min(max, Math.max(min, snapped));
}

export type WheelMode = "zoom" | "tune";
export type WheelAction = "zoom" | "tune";

/**
 * What the wheel does: Ctrl always zooms; otherwise it tunes the active VFO when the setting says
 * so or the pointer is over its passband, and zooms elsewhere.
 */
export function wheelAction(opts: {
  ctrlKey: boolean;
  setting: WheelMode;
  hasVfo: boolean;
  overPassband: boolean;
}): WheelAction {
  if (opts.ctrlKey || !opts.hasVfo) return "zoom";
  return opts.setting === "tune" || opts.overPassband ? "tune" : "zoom";
}

/** Turns wheel pixels into whole tuning steps; the remainder carries over, a reversal resets it. */
export class WheelSteps {
  private acc = 0;

  /** Wheel down (positive pixels) gives negative steps (tune down), wheel up tunes up. */
  add(pixels: number): number {
    const delta = -pixels;
    if (this.acc !== 0 && Math.sign(delta) !== Math.sign(this.acc)) this.acc = 0;
    this.acc += delta;
    const steps = Math.trunc(this.acc / WHEEL_STEP_PX) || 0;
    this.acc -= steps * WHEEL_STEP_PX;
    return steps;
  }

  reset(): void {
    this.acc = 0;
  }
}
