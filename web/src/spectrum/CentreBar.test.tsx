import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { CentreBar } from "./CentreBar";

// 600 px wide bar showing 2.4 MHz around a 100 MHz centre: 100 px = 400 kHz.
function setup(blocked = false) {
  const picture = document.createElement("canvas");
  const tuning = {
    getSnapshot: () => ({ display: { centerHz: 100e6, sampleRate: 2_400_000 } }) as never,
    requestCentre: vi.fn(),
    refuseCentreChange: vi.fn(() => blocked),
  };
  render(
    <CentreBar
      tuning={tuning}
      getSpan={() => ({ startHz: 98.8e6, endHz: 101.2e6 })}
      getPreview={() => picture}
    />,
  );
  const bar = screen.getByRole("button", { name: /Move the device centre/ });
  vi.spyOn(bar, "getBoundingClientRect").mockReturnValue({ width: 600 } as DOMRect);
  return { tuning, bar, picture };
}

beforeEach(() => {
  vi.useFakeTimers();
  Element.prototype.setPointerCapture = vi.fn();
});
afterEach(() => vi.useRealTimers());

it("follows the pointer live and commits the new centre on release", () => {
  const { tuning, bar, picture } = setup();
  fireEvent.pointerDown(bar, { clientX: 300, pointerId: 1, button: 0 });
  fireEvent.pointerMove(bar, { clientX: 400, pointerId: 1 });
  // Preview only: the picture shifts with the pointer, the chip names the target.
  expect(picture.style.transform).toBe("translate3d(100px,0,0)");
  expect(screen.getByTestId("centre-bar-chip")).toHaveTextContent("99.600 MHz");
  expect(tuning.requestCentre).not.toHaveBeenCalled();
  fireEvent.pointerUp(bar, { clientX: 400, pointerId: 1 });
  expect(tuning.requestCentre).toHaveBeenCalledOnce();
  expect(tuning.requestCentre).toHaveBeenCalledWith(99_600_000);
  expect(screen.getByTestId("centre-bar-chip")).not.toBeVisible();
  // The shifted picture stays until the new frames arrive, then goes back to rest.
  expect(picture.style.transform).not.toBe("");
  act(() => vi.advanceTimersByTime(400));
  expect(picture.style.transform).toBe("");
});

it("drags the other way for a higher centre", () => {
  const { tuning, bar } = setup();
  fireEvent.pointerDown(bar, { clientX: 300, pointerId: 1, button: 0 });
  fireEvent.pointerMove(bar, { clientX: 150, pointerId: 1 });
  fireEvent.pointerUp(bar, { clientX: 150, pointerId: 1 });
  expect(tuning.requestCentre).toHaveBeenCalledWith(100_600_000);
});

it("a click or a cancelled drag changes nothing", () => {
  const { tuning, bar, picture } = setup();
  fireEvent.pointerDown(bar, { clientX: 300, pointerId: 1, button: 0 });
  fireEvent.pointerUp(bar, { clientX: 301, pointerId: 1 });
  fireEvent.pointerDown(bar, { clientX: 300, pointerId: 2, button: 0 });
  fireEvent.pointerMove(bar, { clientX: 400, pointerId: 2 });
  fireEvent.pointerCancel(bar, { pointerId: 2 });
  expect(tuning.requestCentre).not.toHaveBeenCalled();
  expect(picture.style.transform).toBe("");
});

it("shows no preview and moves nothing while the tuning is locked or claimed", () => {
  const { tuning, bar, picture } = setup(true);
  fireEvent.pointerDown(bar, { clientX: 300, pointerId: 1, button: 0 });
  fireEvent.pointerMove(bar, { clientX: 400, pointerId: 1 });
  fireEvent.pointerMove(bar, { clientX: 500, pointerId: 1 });
  fireEvent.pointerUp(bar, { clientX: 500, pointerId: 1 });
  // The reason is shown once per gesture, not per move.
  expect(tuning.refuseCentreChange).toHaveBeenCalledOnce();
  expect(picture.style.transform).toBe("");
  expect(tuning.requestCentre).not.toHaveBeenCalled();
});

it("the arrow keys step the centre with the bar focused", () => {
  const { tuning, bar } = setup();
  fireEvent.keyDown(bar, { key: "ArrowRight" });
  expect(tuning.requestCentre).toHaveBeenLastCalledWith(100_200_000);
  fireEvent.keyDown(bar, { key: "ArrowLeft", ctrlKey: true });
  expect(tuning.requestCentre).toHaveBeenLastCalledWith(99_000_000);
});
