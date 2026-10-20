import { describe, expect, it, vi } from "vitest";
import {
  MIN_VIEW_SPAN_HZ,
  ViewController,
  clampSpan,
  deviceSpan,
  isFullSpan,
  panSpan,
  zoomSpan,
  type Span,
} from "./view";

const device = deviceSpan(100e6, 2.4e6); // 98.8 .. 101.2 MHz

describe("zoomSpan", () => {
  it("keeps the anchor frequency under the pointer", () => {
    const next = zoomSpan(device, device, 0.25, 0.5);
    expect(next.endHz - next.startHz).toBeCloseTo(1.2e6);
    // Anchor was 98.8e6 + 0.25 * 2.4e6 = 99.4 MHz and stays at 25 %.
    expect(next.startHz + 0.25 * 1.2e6).toBeCloseTo(99.4e6);
  });

  it("never exceeds the device span", () => {
    const view = { startHz: 99e6, endHz: 100e6 };
    expect(zoomSpan(view, device, 0.5, 100)).toEqual(device);
  });

  it("stops at the minimum span", () => {
    let view: Span = device;
    for (let i = 0; i < 40; i++) view = zoomSpan(view, device, 0.9, 0.5);
    expect(view.endHz - view.startHz).toBeCloseTo(MIN_VIEW_SPAN_HZ);
    expect(view.startHz).toBeGreaterThanOrEqual(device.startHz);
    expect(view.endHz).toBeLessThanOrEqual(device.endHz);
  });

  it("shifts back inside the device span at the edges", () => {
    const view = { startHz: 98.8e6, endHz: 99.8e6 };
    const next = zoomSpan(view, device, 0, 2);
    expect(next.startHz).toBeCloseTo(98.8e6);
    expect(next.endHz).toBeCloseTo(100.8e6);
  });
});

describe("panSpan and clampSpan", () => {
  it("pans and clamps at the device edges", () => {
    const view = { startHz: 99e6, endHz: 100e6 };
    expect(panSpan(view, device, 0.5e6)).toEqual({ startHz: 99.5e6, endHz: 100.5e6 });
    expect(panSpan(view, device, 9e6)).toEqual({ startHz: 100.2e6, endHz: 101.2e6 });
    expect(panSpan(view, device, -9e6)).toEqual({ startHz: 98.8e6, endHz: 99.8e6 });
  });

  it("limits the width to the device span", () => {
    expect(clampSpan({ startHz: 90e6, endHz: 110e6 }, device)).toEqual(device);
  });

  it("handles a device span below the minimum span", () => {
    const tiny = deviceSpan(1e6, 1000);
    expect(clampSpan({ startHz: 0, endHz: 5 }, tiny)).toEqual(tiny);
  });

  it("detects the full span", () => {
    expect(isFullSpan(device, device)).toBe(true);
    expect(isFullSpan({ startHz: 99e6, endHz: 100e6 }, device)).toBe(false);
  });
});

describe("ViewController", () => {
  function setup() {
    let now = 0;
    let cb: (() => void) | null = null;
    const apply = vi.fn();
    const vc = new ViewController({
      apply,
      now: () => now,
      schedule: (f) => {
        cb = f;
        return 1;
      },
      cancel: () => {
        cb = null;
      },
    });
    const frame = (dt = 50) => {
      now += dt;
      const f = cb;
      cb = null;
      f?.();
    };
    vc.setDevice(device);
    return { vc, apply, frame };
  }

  it("coalesces several zooms into one apply per frame", () => {
    const { vc, apply, frame } = setup();
    vc.zoom(0.5, 0.8);
    vc.zoom(0.5, 0.8);
    vc.zoom(0.5, 0.8);
    expect(apply).not.toHaveBeenCalled();
    frame();
    expect(apply).toHaveBeenCalledTimes(1);
    const arg = apply.mock.calls[0]?.[0] as Span;
    expect(arg.endHz - arg.startHz).toBeCloseTo(2.4e6 * 0.512, -1);
  });

  it("applies null when reset to the full span", () => {
    const { vc, apply, frame } = setup();
    vc.zoom(0.5, 0.5);
    frame();
    vc.reset();
    frame();
    expect(apply).toHaveBeenLastCalledWith(null);
    expect(vc.zoomed).toBe(false);
  });

  it("recentres a zoomed view when the device centre changes", () => {
    const { vc, apply, frame } = setup();
    vc.zoom(0.5, 0.25);
    frame();
    vc.setDevice(deviceSpan(433.92e6, 2.4e6));
    frame();
    const arg = apply.mock.calls.at(-1)?.[0] as Span;
    expect((arg.startHz + arg.endHz) / 2).toBeCloseTo(433.92e6);
    expect(arg.endHz - arg.startHz).toBeCloseTo(0.6e6);
  });

  it("does not pan when not zoomed", () => {
    const { vc, apply, frame } = setup();
    vc.panFrom(device, 1e5);
    frame();
    expect(apply).not.toHaveBeenCalled();
  });
});
