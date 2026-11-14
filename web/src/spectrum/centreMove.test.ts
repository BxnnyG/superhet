import { describe, expect, it } from "vitest";
import {
  MAX_CENTRE_HZ,
  MIN_CENTRE_HZ,
  PILL_PX_PER_STEP,
  centreStepHz,
  clampCentre,
  dragSteps,
  keySteps,
  niceFloor,
  panCentre,
  previewShiftPx,
  stepCentre,
  wheelPixels,
} from "./centreMove";
import { WheelSteps } from "./gestures";

const plain = { shift: false, ctrl: false };

describe("centre step", () => {
  it("rounds down to a 1-2-5 number", () => {
    expect(niceFloor(240_000)).toBe(200_000);
    expect(niceFloor(999)).toBe(500);
    expect(niceFloor(1_000)).toBe(1_000);
    expect(niceFloor(0)).toBe(1);
  });

  it("is a tenth of the span, Shift fine, Ctrl coarse", () => {
    expect(centreStepHz(2_400_000, plain)).toBe(200_000);
    expect(centreStepHz(2_400_000, { shift: true, ctrl: false })).toBe(20_000);
    expect(centreStepHz(2_400_000, { shift: false, ctrl: true })).toBe(1_000_000);
    expect(centreStepHz(250_000, plain)).toBe(20_000);
    // Never finer than 1 kHz, and a missing span still gives a step.
    expect(centreStepHz(20_000, { shift: true, ctrl: false })).toBe(1_000);
    expect(centreStepHz(0, plain)).toBe(100_000);
  });

  it("steps the centre up and down and stays in range", () => {
    expect(stepCentre(100_000_000, 1, 2_400_000, plain)).toBe(100_200_000);
    expect(stepCentre(100_000_000, -3, 2_400_000, { shift: true, ctrl: false })).toBe(99_940_000);
    expect(stepCentre(500, -1, 2_400_000, plain)).toBe(MIN_CENTRE_HZ);
    expect(stepCentre(MAX_CENTRE_HZ, 5, 2_400_000, plain)).toBe(MAX_CENTRE_HZ);
    expect(clampCentre(100.4)).toBe(MIN_CENTRE_HZ);
    expect(clampCentre(145_000_000.6)).toBe(145_000_001);
  });
});

describe("wheel on the centre", () => {
  it("turns wheel pixels into steps: up goes up, the remainder carries over", () => {
    const wheel = new WheelSteps();
    expect(wheel.add(-100)).toBe(1);
    expect(wheel.add(100)).toBe(-1);
    // Small trackpad deltas add up to one step.
    expect(wheel.add(-30) + wheel.add(-30) + wheel.add(-30)).toBe(1);
  });

  it("scales lines and pages to pixels", () => {
    expect(wheelPixels({ deltaY: 3, deltaMode: 0 })).toBe(3);
    expect(wheelPixels({ deltaY: 3, deltaMode: 1 })).toBe(120);
    expect(wheelPixels({ deltaY: 1, deltaMode: 2 })).toBe(400);
  });
});

describe("drag on the pill", () => {
  it("counts whole steps; right goes up in frequency", () => {
    expect(dragSteps(PILL_PX_PER_STEP - 1)).toBe(0);
    expect(dragSteps(PILL_PX_PER_STEP)).toBe(1);
    expect(dragSteps(-PILL_PX_PER_STEP * 2.5)).toBe(-2);
    expect(Object.is(dragSteps(-3), 0)).toBe(true);
  });
});

describe("pan bar", () => {
  it("moves the centre against the drag by the share of the span travelled", () => {
    // 600 px wide bar showing 2.4 MHz: 100 px = 400 kHz; the picture follows the pointer.
    expect(panCentre(100_000_000, 100, 600, 2_400_000)).toBe(99_600_000);
    expect(panCentre(100_000_000, -150, 600, 2_400_000)).toBe(100_600_000);
    expect(panCentre(100_000_000, 0, 600, 2_400_000)).toBe(100_000_000);
    expect(panCentre(100_000_000, 50, 0, 2_400_000)).toBe(100_000_000);
    expect(panCentre(5_000, 600, 600, 2_400_000)).toBe(MIN_CENTRE_HZ);
  });

  it("previews the shift of the picture as the inverse of the centre change", () => {
    expect(previewShiftPx(-400_000, 600, 2_400_000)).toBe(100);
    expect(previewShiftPx(1_000_000, 600, 2_400_000)).toBe(-250);
    expect(previewShiftPx(1, 0, 2_400_000)).toBe(0);
    // Dragging 100 px and shifting the picture by 100 px describe the same move.
    const hz = panCentre(100e6, 100, 600, 2_400_000);
    expect(previewShiftPx(hz - 100e6, 600, 2_400_000)).toBe(100);
  });
});

describe("keys on the pill and the bar", () => {
  it("maps arrows to steps with the modifiers, page keys to coarse steps", () => {
    const key = (k: string, shiftKey = false, ctrlKey = false) =>
      keySteps({ key: k, shiftKey, ctrlKey });
    expect(key("ArrowRight")).toEqual({ steps: 1, mods: plain });
    expect(key("ArrowLeft", true)).toEqual({ steps: -1, mods: { shift: true, ctrl: false } });
    expect(key("ArrowUp", false, true)?.mods.ctrl).toBe(true);
    expect(key("PageDown")).toEqual({ steps: -1, mods: { shift: false, ctrl: true } });
    expect(key("a")).toBeNull();
  });
});
