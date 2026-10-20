import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { LevelController } from "./levels";
import { DisplayControls } from "./DisplayControls";

const noView = { current: { reset: () => undefined, zoom: () => undefined } };

/** A frame whose noise floor is around -40 dB and peaks reach about -15 dB. */
function frame() {
  const bins = new Uint8Array(512).fill(200);
  bins.fill(235, 0, 8);
  return { bins, minDb: -120, maxDb: 0 };
}

beforeEach(() => {
  vi.useFakeTimers();
  window.localStorage.clear();
});
afterEach(() => {
  vi.useRealTimers();
});

it("with automatic levels on, the range and sliders track incoming frames after mount", () => {
  const controller = new LevelController({ auto: true, manualLowDb: -100, manualHighDb: -20 });
  render(<DisplayControls controller={controller} info={null} viewActions={noView} />);
  expect(screen.getByRole("slider", { name: "Floor" })).toHaveAttribute(
    "aria-valuetext",
    "-100 dB",
  );

  act(() => {
    controller.onFrame(frame(), 1000);
    vi.advanceTimersByTime(600);
  });
  expect(controller.range.lowDb).toBeGreaterThan(-60);
  expect(screen.getByRole("switch", { name: "Automatic levels" })).toHaveAttribute(
    "aria-checked",
    "true",
  );
  const floor = Number(controller.range.lowDb.toFixed(0));
  expect(screen.getByRole("slider", { name: "Floor" })).toHaveAttribute(
    "aria-valuetext",
    `${floor} dB`,
  );
  // Nothing is persisted by merely following the frames.
  expect(window.localStorage.getItem("superhet.levels.v1")).toBeNull();
});

it("the sliders are locked while auto is on, so a stray click cannot persist a manual range", () => {
  const controller = new LevelController({ auto: true, manualLowDb: -100, manualHighDb: -20 });
  render(<DisplayControls controller={controller} info={null} viewActions={noView} />);
  expect(screen.getByRole("slider", { name: "Floor" })).toBeDisabled();
  expect(screen.getByRole("slider", { name: "Ceiling" })).toBeDisabled();

  // Switch to manual: sliders unlock and the choice is persisted; back to auto tracks again.
  fireEvent.click(screen.getByRole("switch", { name: "Automatic levels" }));
  expect(controller.auto).toBe(false);
  const floor = screen.getByRole("slider", { name: "Floor" });
  expect(floor).toBeEnabled();
  fireEvent.change(floor, { target: { value: "-110" } });
  expect(controller.range.lowDb).toBe(-110);

  fireEvent.click(screen.getByRole("switch", { name: "Automatic levels" }));
  expect(controller.auto).toBe(true);
  act(() => {
    controller.onFrame(frame(), 5000);
  });
  expect(controller.range.lowDb).toBeGreaterThan(-60);
});
