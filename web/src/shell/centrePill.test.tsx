import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";
import { CentrePill } from "./CentrePill";
import { fake, vfo } from "./fixtures";

// 2.048 MS/s span: one step is 200 kHz, Shift 20 kHz, Ctrl 1 MHz; the centre is 100 MHz.
function setup() {
  const view = fake([vfo(1)], 1);
  render(<CentrePill r={view.r} />);
  const pill = screen.getByRole("button", { name: /Centre 100\.000/ });
  return { ...view, pill };
}

beforeEach(() => {
  Element.prototype.setPointerCapture = vi.fn();
});

it("the mouse wheel steps the centre: up goes up, Shift fine, Ctrl coarse", () => {
  const { fns, pill } = setup();
  fireEvent.wheel(pill, { deltaY: -100 });
  expect(fns.requestCentre).toHaveBeenLastCalledWith(100_200_000);
  fireEvent.wheel(pill, { deltaY: 100 });
  expect(fns.requestCentre).toHaveBeenLastCalledWith(99_800_000);
  fireEvent.wheel(pill, { deltaY: -100, shiftKey: true });
  expect(fns.requestCentre).toHaveBeenLastCalledWith(100_020_000);
  fireEvent.wheel(pill, { deltaY: -100, ctrlKey: true });
  expect(fns.requestCentre).toHaveBeenLastCalledWith(101_000_000);
  // A small nudge is less than a step and does nothing yet.
  fns.requestCentre.mockClear();
  fireEvent.wheel(pill, { deltaY: -20 });
  expect(fns.requestCentre).not.toHaveBeenCalled();
});

it("the arrow keys step the centre while the pill has focus", () => {
  const { fns, pill } = setup();
  fireEvent.keyDown(pill, { key: "ArrowRight" });
  expect(fns.requestCentre).toHaveBeenLastCalledWith(100_200_000);
  fireEvent.keyDown(pill, { key: "ArrowLeft", shiftKey: true });
  expect(fns.requestCentre).toHaveBeenLastCalledWith(99_980_000);
  fireEvent.keyDown(pill, { key: "PageUp" });
  expect(fns.requestCentre).toHaveBeenLastCalledWith(101_000_000);
  fns.requestCentre.mockClear();
  fireEvent.keyDown(pill, { key: "a" });
  expect(fns.requestCentre).not.toHaveBeenCalled();
});

it("dragging the pill moves the centre by whole steps and does not open the settings", () => {
  const { fns, pill } = setup();
  fireEvent.pointerDown(pill, { clientX: 100, pointerId: 1, button: 0 });
  // Within the slop it is still a click.
  fireEvent.pointerMove(pill, { clientX: 102, pointerId: 1 });
  expect(fns.requestCentre).not.toHaveBeenCalled();
  fireEvent.pointerMove(pill, { clientX: 150, pointerId: 1 });
  expect(fns.requestCentre).toHaveBeenLastCalledWith(100_400_000);
  fireEvent.pointerMove(pill, { clientX: 20, pointerId: 1 });
  expect(fns.requestCentre).toHaveBeenLastCalledWith(99_400_000);
  fireEvent.pointerUp(pill, { clientX: 20, pointerId: 1 });
  fireEvent.click(pill);
  expect(screen.queryByRole("dialog", { name: "Source settings" })).toBeNull();
  // A plain click afterwards still opens it.
  fireEvent.click(pill);
  expect(screen.getByRole("dialog", { name: "Source settings" })).toBeInTheDocument();
});

it("says why instead of moving when the tuning is locked or claimed", () => {
  const { fns, pill } = setup();
  fns.refuseCentreChange.mockReturnValue(true);
  fireEvent.wheel(pill, { deltaY: -100 });
  fireEvent.keyDown(pill, { key: "ArrowRight" });
  fireEvent.pointerDown(pill, { clientX: 100, pointerId: 1, button: 0 });
  fireEvent.pointerMove(pill, { clientX: 160, pointerId: 1 });
  fireEvent.pointerMove(pill, { clientX: 220, pointerId: 1 });
  expect(fns.refuseCentreChange).toHaveBeenCalled();
  expect(fns.requestCentre).not.toHaveBeenCalled();
});
