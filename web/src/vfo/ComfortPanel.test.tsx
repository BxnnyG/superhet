import { fireEvent, render, screen } from "@testing-library/react";
import { expect, it, vi } from "vitest";
import type { Vfo, VfoPatch } from "./api";
import { ComfortPanel } from "./ComfortPanel";

const vfo = (over: Partial<Vfo> = {}): Vfo => ({
  id: 3,
  freqHz: 7_100_000,
  mode: "lsb",
  bandwidthHz: 2_400,
  squelchDbfs: null,
  deemphasis: null,
  offsetHz: 0,
  status: "active",
  ...over,
});

const setup = (v: Vfo) => {
  const onUpdate = vi.fn<(id: number, patch: VfoPatch) => void>();
  render(<ComfortPanel vfo={v} onUpdate={onUpdate} />);
  return onUpdate;
};

it("starts with every block off and switches one on with its default strength", () => {
  const onUpdate = setup(vfo());
  for (const name of ["Noise blanker", "Notch", "Auto notch", "Noise reduction"]) {
    expect(screen.getByRole("button", { name })).toHaveAttribute("aria-pressed", "false");
  }
  fireEvent.click(screen.getByRole("button", { name: "Noise reduction" }));
  expect(onUpdate).toHaveBeenCalledWith(3, { comfort: { nr: 0.5 } });
});

it("shows a strength slider for a block that is on and sends the new strength", () => {
  const onUpdate = setup(vfo({ comfort: { nb: 0.5 } }));
  expect(screen.getByRole("button", { name: "Noise blanker" })).toHaveAttribute(
    "aria-pressed",
    "true",
  );
  fireEvent.change(screen.getByRole("slider", { name: "Blanker strength" }), {
    target: { value: "80" },
  });
  expect(onUpdate).toHaveBeenCalledWith(3, { comfort: { nb: 0.8 } });
  fireEvent.click(screen.getByRole("button", { name: "Noise blanker" }));
  expect(onUpdate).toHaveBeenLastCalledWith(3, { comfort: {} });
});

it("keeps the other blocks when one changes", () => {
  const onUpdate = setup(vfo({ comfort: { nr: 0.7, autoNotch: 0.3 } }));
  fireEvent.click(screen.getByRole("button", { name: "Notch" }));
  expect(onUpdate).toHaveBeenCalledWith(3, {
    comfort: { nr: 0.7, autoNotch: 0.3, notch: { freqHz: 1000, widthHz: 60 } },
  });
});

it("moves the notch marker with the arrow keys and sets its width", () => {
  const onUpdate = setup(vfo({ comfort: { notch: { freqHz: 1500, widthHz: 60 } } }));
  const marker = screen.getByRole("slider", { name: "Notch frequency" });
  expect(marker).toHaveAttribute("aria-valuenow", "1500");
  fireEvent.keyDown(marker, { key: "ArrowRight" });
  expect(onUpdate).toHaveBeenLastCalledWith(3, {
    comfort: { notch: { freqHz: 1510, widthHz: 60 } },
  });
  fireEvent.keyDown(marker, { key: "ArrowLeft", shiftKey: true });
  expect(onUpdate).toHaveBeenLastCalledWith(3, {
    comfort: { notch: { freqHz: 1400, widthHz: 60 } },
  });
  fireEvent.change(screen.getByRole("slider", { name: "Notch width" }), {
    target: { value: "200" },
  });
  expect(onUpdate).toHaveBeenLastCalledWith(3, {
    comfort: { notch: { freqHz: 1500, widthHz: 200 } },
  });
});

it("drags the marker across the audio passband", () => {
  const onUpdate = setup(vfo({ comfort: { notch: { freqHz: 1000, widthHz: 60 } } }));
  const strip = screen.getByTestId("notch-strip");
  strip.getBoundingClientRect = () =>
    ({
      left: 100,
      width: 200,
      top: 0,
      height: 30,
      right: 300,
      bottom: 30,
      x: 100,
      y: 0,
    }) as DOMRect;
  // LSB passband 300..2700 Hz over 200 px: the middle is 1500 Hz.
  fireEvent.pointerDown(strip, { clientX: 200, pointerId: 1 });
  expect(onUpdate).toHaveBeenLastCalledWith(3, {
    comfort: { notch: { freqHz: 1500, widthHz: 60 } },
  });
});

it("says why there is nothing to switch on for WFM", () => {
  setup(vfo({ mode: "wfm", bandwidthHz: 180_000 }));
  expect(screen.queryByRole("button", { name: "Notch" })).toBeNull();
  expect(screen.getByText(/not WFM/)).toBeInTheDocument();
});
