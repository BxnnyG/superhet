// Maths of the "move the device centre" gestures: mouse wheel and arrow keys on the centre pill,
// press-and-drag on the pill, and the pan bar above the waterfall. Pure, so it is tested without
// a browser; the gesture wiring lives in shell/CentrePill.tsx and spectrum/CentreBar.tsx.

/** Lowest / highest centre a gesture may ask for (the server validates the real device range). */
export const MIN_CENTRE_HZ = 1_000;
export const MAX_CENTRE_HZ = 10_000_000_000;

/** Pointer travel on the pill that counts as one step. */
export const PILL_PX_PER_STEP = 24;
/** Travel before a press on the pill is a drag and no longer a click. */
export const DRAG_SLOP_PX = 4;

export interface CentreMods {
  /** Fine: a hundredth of the span. */
  shift: boolean;
  /** Coarse: half the span. */
  ctrl: boolean;
}

/** Largest 1-2-5 number not above `x` (`x` > 0). */
export function niceFloor(x: number): number {
  if (!(x > 0)) return 1;
  const exp = 10 ** Math.floor(Math.log10(x));
  const m = x / exp;
  return (m >= 5 ? 5 : m >= 2 ? 2 : 1) * exp;
}

/**
 * One step of the centre in Hz: a tenth of the span (200 kHz on a 2.4 MHz span), Shift a hundredth
 * (fine, at least 1 kHz), Ctrl half (coarse). Always a 1-2-5 number so the centre stays tidy.
 */
export function centreStepHz(spanHz: number, mods: CentreMods): number {
  const span = spanHz > 0 ? spanHz : 1_000_000;
  if (mods.ctrl) return niceFloor(span / 2);
  if (mods.shift) return Math.max(1_000, niceFloor(span / 100));
  return Math.max(1_000, niceFloor(span / 10));
}

export function clampCentre(hz: number): number {
  return Math.round(Math.min(MAX_CENTRE_HZ, Math.max(MIN_CENTRE_HZ, hz)));
}

/** The centre `steps` steps away (positive = up in frequency). */
export function stepCentre(
  centreHz: number,
  steps: number,
  spanHz: number,
  mods: CentreMods,
): number {
  return clampCentre(centreHz + steps * centreStepHz(spanHz, mods));
}

/** Whole steps for a horizontal drag on the pill; dragging right goes up in frequency. */
export function dragSteps(dxPx: number): number {
  return Math.trunc(dxPx / PILL_PX_PER_STEP) || 0;
}

/**
 * Centre for a drag on the pan bar: the picture follows the pointer, so dragging right moves the
 * centre down by the share of the visible span the pointer travelled.
 */
export function panCentre(startHz: number, dxPx: number, widthPx: number, spanHz: number): number {
  if (!(widthPx > 0) || !(spanHz > 0)) return clampCentre(startHz);
  return clampCentre(startHz - (dxPx / widthPx) * spanHz);
}

/** Pixels the picture has to shift for a centre change (live preview of a pan). */
export function previewShiftPx(deltaHz: number, widthPx: number, spanHz: number): number {
  if (!(widthPx > 0) || !(spanHz > 0)) return 0;
  return -(deltaHz / spanHz) * widthPx;
}

/** Pixels of a wheel event, whatever its unit (lines and pages are scaled to pixels). */
export function wheelPixels(e: { deltaY: number; deltaMode: number }): number {
  if (e.deltaMode === 1) return e.deltaY * 40;
  if (e.deltaMode === 2) return e.deltaY * 400;
  return e.deltaY;
}

/** Arrow / page keys on the focused pill or bar: signed steps and modifiers, or null. */
export function keySteps(e: {
  key: string;
  shiftKey: boolean;
  ctrlKey: boolean;
}): { steps: number; mods: CentreMods } | null {
  const mods = { shift: e.shiftKey, ctrl: e.ctrlKey };
  switch (e.key) {
    case "ArrowRight":
    case "ArrowUp":
      return { steps: 1, mods };
    case "ArrowLeft":
    case "ArrowDown":
      return { steps: -1, mods };
    case "PageUp":
      return { steps: 1, mods: { shift: false, ctrl: true } };
    case "PageDown":
      return { steps: -1, mods: { shift: false, ctrl: true } };
    default:
      return null;
  }
}
