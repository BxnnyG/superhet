import { fireEvent, render, screen } from "@testing-library/react";
import { expect, it, vi } from "vitest";
import type { Vfo } from "../vfo/api";
import { ReceiverFrequency } from "./ReceiverFrequency";

const vfo: Vfo = {
  id: 2,
  freqHz: 145_500_000,
  mode: "nfm",
  bandwidthHz: 12_500,
  squelchDbfs: null,
  deemphasis: null,
  offsetHz: 0,
  status: "active",
};

function setup(selected: Vfo | null) {
  const onCenter = vi.fn();
  const onVfo = vi.fn();
  render(
    <ReceiverFrequency
      centerHz={100_000_000}
      selected={selected}
      onCenter={onCenter}
      onVfo={onVfo}
    />,
  );
  return { onCenter, onVfo };
}

it("tunes the device centre when no VFO is selected", () => {
  const { onCenter, onVfo } = setup(null);
  expect(screen.getByText("Centre")).toBeInTheDocument();
  const d = screen.getByRole("spinbutton", { name: "100 kHz digit" });
  fireEvent.keyDown(d, { key: "ArrowUp" });
  expect(onCenter).toHaveBeenCalledWith(100_100_000);
  expect(onVfo).not.toHaveBeenCalled();
});

it("tunes the selected VFO instead of the centre", () => {
  const { onCenter, onVfo } = setup(vfo);
  expect(screen.getByRole("group", { name: "VFO frequency" })).toBeInTheDocument();
  expect(screen.getByText(/VFO 2/)).toBeInTheDocument();
  fireEvent.wheel(screen.getByRole("spinbutton", { name: "10 kHz digit" }), { deltaY: -100 });
  expect(onVfo).toHaveBeenCalledWith(2, 145_510_000);
  expect(onCenter).not.toHaveBeenCalled();
});

it("typed entry goes to the selected VFO as well", () => {
  const { onCenter, onVfo } = setup(vfo);
  fireEvent.keyDown(screen.getByRole("spinbutton", { name: "1 kHz digit" }), { key: "Enter" });
  const input = screen.getByRole("textbox");
  fireEvent.change(input, { target: { value: "145.6" } });
  fireEvent.keyDown(input, { key: "Enter" });
  expect(onVfo).toHaveBeenCalledWith(2, 145_600_000);
  expect(onCenter).not.toHaveBeenCalled();
});
