import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { AnnotationLayer } from "./annotations";
import { SpectrumInteraction, type InteractionOptions } from "./interaction";
import type { Marker } from "../vfo/model";
import { ViewController } from "./view";

// Plot area: x 0..1000 CSS px (no left pad) mapped to 100.0..101.0 MHz, i.e. 1 px = 1 kHz.
const START = 100_000_000;
const WIDTH = 1_000_000;
const marker: Marker = {
  id: 1,
  freqHz: START + 500_000,
  bandwidthHz: 100_000,
  mode: "wfm",
  paused: false,
};

function setup(over: Partial<InteractionOptions> = {}, markers: Marker[] = [marker]) {
  const canvas = document.createElement("canvas");
  canvas.getBoundingClientRect = () => new DOMRect(0, 0, 1000, 300);
  const applied: unknown[] = [];
  const view = new ViewController({
    apply: (v) => applied.push(v),
    minIntervalMs: 0,
    schedule: (cb) => {
      cb();
      return 1;
    },
    cancel: () => undefined,
  });
  view.setDevice({ startHz: START, endHz: START + WIDTH });
  const calls = {
    tune: vi.fn(),
    select: vi.fn(),
    move: vi.fn(),
    create: vi.fn(),
    step: vi.fn(),
    bandwidth: vi.fn(),
  };
  const it = new SpectrumInteraction({
    canvas,
    leftPad: 0,
    view,
    onTune: calls.tune,
    onVfoSelect: calls.select,
    onVfoMove: calls.move,
    snap: (hz) => Math.round(hz / 1000) * 1000,
    onVfoCreate: calls.create,
    annotations: new AnnotationLayer(),
    onAnnotationTune: () => undefined,
    getWheelMode: () => "zoom",
    onVfoStep: calls.step,
    onBandwidth: calls.bandwidth,
    axis: () => ({ top: 282, bottom: 300 }),
    ...over,
  });
  it.setFrameSpan(START + WIDTH / 2, WIDTH);
  it.setVfos(markers, 1);
  return { canvas, view, it, calls, applied };
}

function pointer(canvas: HTMLElement, type: string, x: number, y = 100, init: MouseEventInit = {}) {
  canvas.dispatchEvent(
    new MouseEvent(type, { clientX: x, clientY: y, button: 0, bubbles: true, ...init }),
  );
}

function wheel(canvas: HTMLElement, x: number, deltaY: number, init: WheelEventInit = {}) {
  const e = new WheelEvent("wheel", {
    clientX: x,
    clientY: 100,
    deltaY,
    cancelable: true,
    ...init,
  });
  canvas.dispatchEvent(e);
  return e;
}

beforeEach(() => {
  vi.useFakeTimers();
  vi.stubGlobal("requestAnimationFrame", () => 0);
  vi.stubGlobal("cancelAnimationFrame", () => undefined);
});
afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

describe("click versus drag", () => {
  it("a press that moves up to 4 px is still a click and tunes", () => {
    const { canvas, calls, it: ia } = setup();
    pointer(canvas, "pointerdown", 100);
    pointer(canvas, "pointermove", 104);
    pointer(canvas, "pointerup", 104);
    vi.advanceTimersByTime(300);
    expect(calls.move).toHaveBeenCalledWith(1, START + 104_000);
    ia.destroy();
  });

  it("a longer drag on empty spectrum pans a zoomed view and does not tune", () => {
    const { canvas, view, calls, applied, it: ia } = setup();
    view.zoom(0.5, 0.2);
    applied.length = 0;
    const before = view.current();
    pointer(canvas, "pointerdown", 100);
    pointer(canvas, "pointermove", 140);
    pointer(canvas, "pointerup", 140);
    vi.advanceTimersByTime(300);
    const after = view.current();
    expect(after?.startHz).toBeLessThan(before?.startHz ?? 0);
    expect(calls.move).not.toHaveBeenCalled();
    expect(calls.tune).not.toHaveBeenCalled();
    ia.destroy();
  });

  it("a plain drag on the frequency axis pans too, even over a VFO", () => {
    const { canvas, view, calls, it: ia } = setup();
    view.zoom(0.5, 0.2);
    const before = view.current();
    pointer(canvas, "pointerdown", 500, 290);
    pointer(canvas, "pointermove", 540, 290);
    pointer(canvas, "pointerup", 540, 290);
    expect(view.current()?.startHz).toBeLessThan(before?.startHz ?? 0);
    expect(calls.move).not.toHaveBeenCalled();
    ia.destroy();
  });

  it("a click on the axis tunes the active VFO", () => {
    const { canvas, calls, it: ia } = setup();
    pointer(canvas, "pointerdown", 200, 290);
    pointer(canvas, "pointerup", 200, 290);
    vi.advanceTimersByTime(300);
    expect(calls.move).toHaveBeenCalledWith(1, START + 200_000);
    ia.destroy();
  });
});

describe("passband edge drag", () => {
  it("drags the upper edge to a new symmetric bandwidth, snapped to 100 Hz", () => {
    const { canvas, calls, it: ia } = setup();
    // Upper edge at x = 550; pull it to 560.04 kHz offset from the carrier at 500.
    pointer(canvas, "pointerdown", 550);
    pointer(canvas, "pointermove", 560);
    expect(calls.bandwidth).toHaveBeenLastCalledWith(1, 120_000);
    pointer(canvas, "pointermove", 545);
    expect(calls.bandwidth).toHaveBeenLastCalledWith(1, 100_000);
    pointer(canvas, "pointermove", 590);
    expect(calls.bandwidth).toHaveBeenLastCalledWith(1, 180_000);
    pointer(canvas, "pointerup", 590);
    expect(calls.move).not.toHaveBeenCalled();
    expect(calls.tune).not.toHaveBeenCalled();
    ia.destroy();
  });

  it("clamps to the preset limits of the mode", () => {
    const { canvas, calls, it: ia } = setup();
    pointer(canvas, "pointerdown", 450);
    pointer(canvas, "pointermove", 100);
    expect(calls.bandwidth).toHaveBeenLastCalledWith(1, 200_000);
    ia.destroy();
  });

  it("shows ew-resize on an edge and grab on the body", () => {
    const { canvas, it: ia } = setup();
    pointer(canvas, "pointermove", 550);
    expect(canvas.style.cursor).toBe("ew-resize");
    pointer(canvas, "pointermove", 520);
    expect(canvas.style.cursor).toBe("grab");
    pointer(canvas, "pointermove", 200);
    expect(canvas.style.cursor).toBe("");
    ia.destroy();
  });

  it("does not grab the edge of a VFO that is not selected", () => {
    const other: Marker = { ...marker, id: 2, freqHz: START + 200_000 };
    const { canvas, calls, it: ia } = setup({}, [marker, other]);
    pointer(canvas, "pointerdown", 250);
    pointer(canvas, "pointermove", 280);
    expect(calls.bandwidth).not.toHaveBeenCalled();
    ia.destroy();
  });
});

describe("wheel routing", () => {
  it("zooms everywhere, also over the passband of the active VFO", () => {
    const { canvas, calls, view, it: ia } = setup();
    wheel(canvas, 100, -100);
    expect(view.zoomed).toBe(true);
    ia.destroy();
    const s = setup();
    s.canvas.dispatchEvent(new WheelEvent("wheel", { clientX: 520, clientY: 100, deltaY: -100 }));
    wheel(s.canvas, 520, -100);
    expect(s.view.zoomed).toBe(true);
    expect(s.calls.step).not.toHaveBeenCalled();
    expect(calls.step).not.toHaveBeenCalled();
    s.it.destroy();
  });

  it("Shift+wheel tunes the active VFO, also away from its passband", () => {
    const { canvas, calls, view, it: ia } = setup();
    wheel(canvas, 520, -100, { shiftKey: true });
    expect(calls.step).toHaveBeenLastCalledWith(1, 1);
    wheel(canvas, 520, 100, { shiftKey: true });
    expect(calls.step).toHaveBeenLastCalledWith(1, -1);
    wheel(canvas, 100, -100, { shiftKey: true });
    expect(calls.step).toHaveBeenLastCalledWith(1, 1);
    expect(view.zoomed).toBe(false);
    ia.destroy();
  });

  it("Shift+wheel works when the browser reports it as a horizontal delta", () => {
    const { canvas, calls, it: ia } = setup();
    wheel(canvas, 520, 0, { shiftKey: true, deltaX: -100 });
    expect(calls.step).toHaveBeenLastCalledWith(1, 1);
    ia.destroy();
  });

  it("Ctrl+wheel over the passband zooms", () => {
    const { canvas, calls, view, it: ia } = setup();
    wheel(canvas, 520, -100, { ctrlKey: true });
    expect(view.zoomed).toBe(true);
    expect(calls.step).not.toHaveBeenCalled();
    ia.destroy();
  });

  it("tune mode tunes everywhere except with Ctrl", () => {
    const { canvas, calls, view, it: ia } = setup({ getWheelMode: () => "tune" });
    const e = wheel(canvas, 100, -100);
    expect(e.defaultPrevented).toBe(true);
    expect(calls.step).toHaveBeenLastCalledWith(1, 1);
    expect(view.zoomed).toBe(false);
    wheel(canvas, 100, -100, { ctrlKey: true });
    expect(view.zoomed).toBe(true);
    ia.destroy();
  });

  it("zooms over the axis", () => {
    const { canvas, calls, view, it: ia } = setup();
    canvas.dispatchEvent(
      new WheelEvent("wheel", { clientX: 520, clientY: 290, deltaY: -100, cancelable: true }),
    );
    expect(view.zoomed).toBe(true);
    expect(calls.step).not.toHaveBeenCalled();
    ia.destroy();
  });
});
