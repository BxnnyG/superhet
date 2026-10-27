// Pure helpers for the spectrum gestures: click/drag threshold, passband edge dragging and the
// routing of the mouse wheel between zooming and tuning. The pointer plumbing lives in
// interaction.ts; keeping the rules here makes them testable without a canvas.

import type { VfoMode } from "../vfo/api";
import { bandwidthRange, snapBandwidth } from "../vfo/bandwidth";
import { SSB_LOW_CUT_HZ, passbandRange, type Marker } from "../vfo/model";

/** Distance in CSS pixels below which a press counts as a click, not a drag. */
export const CLICK_SLOP_PX = 4;
/** Grab tolerance around a VFO centre line or passband edge (CSS px). */
export const VFO_GRAB_PX = 5;
/** Larger grab zone for fingers (CSS px); a mouse or pen keeps `VFO_GRAB_PX`. */
export const TOUCH_GRAB_PX = 14;
/** Accumulated wheel pixels per tuning step (a mouse notch is ~100, a trackpad sends many small). */
export const WHEEL_STEP_PX = 80;

export function exceedsSlop(dxPx: number, slopPx = CLICK_SLOP_PX): boolean {
  return Math.abs(dxPx) > slopPx;
}

export type PassbandEdge = "lo" | "hi";

/** Which side of the carrier a mode occupies: AM/FM/CW both, USB upper, LSB lower. */
export type Sideband = "both" | "upper" | "lower";

export function sidebandOf(mode: VfoMode): Sideband {
  return mode === "usb" ? "upper" : mode === "lsb" ? "lower" : "both";
}

/** The edges of a passband that can be dragged: both, or only the outer one for USB / LSB. */
export function editableEdges(mode: VfoMode): PassbandEdge[] {
  const side = sidebandOf(mode);
  return side === "upper" ? ["hi"] : side === "lower" ? ["lo"] : ["lo", "hi"];
}

/**
 * Which edge of the (selected) VFO passband lies under `hz`, if any. Two-sided passbands offer
 * both edges (the other one mirrors); USB / LSB only their outer edge, the inner one is the fixed
 * low cut. Narrow passbands (edges closer than the grab zone to the centre line) and the centre
 * line itself keep the centre grab.
 */
export function hitPassbandEdge(m: Marker, hz: number, slopHz: number): PassbandEdge | null {
  if (Math.abs(hz - m.freqHz) <= slopHz) return null;
  const { loHz, hiHz } = passbandRange(m.mode, m.freqHz, m.bandwidthHz);
  const candidates = editableEdges(m.mode);
  let best: PassbandEdge | null = null;
  let bestDist = Infinity;
  for (const edge of candidates) {
    const edgeHz = edge === "lo" ? loHz : hiHz;
    // An edge that is too close to the centre line would swallow the centre grab.
    if (Math.abs(edgeHz - m.freqHz) <= slopHz * 1.5) continue;
    const dist = Math.abs(hz - edgeHz);
    if (dist <= slopHz && dist < bestDist) {
      best = edge;
      bestDist = dist;
    }
  }
  return best;
}

/** True when `hz` is inside the passband of `m` (or within `slopHz` of its centre line). */
export function isOverPassband(m: Marker, hz: number, slopHz: number): boolean {
  const { loHz, hiHz } = passbandRange(m.mode, m.freqHz, m.bandwidthHz);
  return (hz >= loHz && hz <= hiHz) || Math.abs(hz - m.freqHz) <= slopHz;
}

/**
 * Bandwidth after dragging an edge to `pointerHz`: twice the distance to the carrier for
 * two-sided modes (the other edge mirrors); for USB / LSB the distance of the outer edge minus
 * the fixed low cut (the passband is `SSB_LOW_CUT_HZ` .. `SSB_LOW_CUT_HZ + bandwidth`). Snapped
 * (100 Hz when narrow, coarser when wide) and clamped to what the mode and the device rate allow;
 * any value in that range is reachable, the presets are only shortcuts.
 */
export function bandwidthFromEdge(
  mode: VfoMode,
  pointerHz: number,
  freqHz: number,
  sideband: Sideband = sidebandOf(mode),
  sampleRateHz?: number | null,
): number {
  const offset =
    sideband === "upper"
      ? pointerHz - freqHz - SSB_LOW_CUT_HZ
      : sideband === "lower"
        ? freqHz - pointerHz - SSB_LOW_CUT_HZ
        : 2 * Math.abs(pointerHz - freqHz);
  const { min, max } = bandwidthRange(mode, sampleRateHz);
  return Math.min(max, Math.max(min, snapBandwidth(Math.max(0, offset))));
}

export type WheelMode = "zoom" | "tune";
export type WheelAction = "zoom" | "tune" | "bandwidth";

/**
 * What the wheel does: with no VFO it zooms; Ctrl always zooms; Shift over the active VFO's
 * passband changes its bandwidth; Shift elsewhere and Alt tune the active VFO; a plain wheel
 * follows the setting (default: zoom everywhere, even over a wide passband).
 */
export function wheelAction(opts: {
  ctrlKey: boolean;
  shiftKey: boolean;
  altKey?: boolean;
  setting: WheelMode;
  hasVfo: boolean;
  /** The pointer is over the active VFO's passband. */
  overPassband?: boolean;
}): WheelAction {
  if (opts.ctrlKey || !opts.hasVfo) return "zoom";
  if (opts.shiftKey && opts.overPassband) return "bandwidth";
  return opts.shiftKey || opts.altKey || opts.setting === "tune" ? "tune" : "zoom";
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
