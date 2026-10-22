import { fireEvent, render, screen, within } from "@testing-library/react";
import { expect, it, vi } from "vitest";
import type { Vfo } from "../vfo/api";
import { ReceiverFrequency } from "./ReceiverFrequency";

const vfo = (over: Partial<Vfo> = {}): Vfo => ({
  id: 2,
  freqHz: 100_000_000,
  mode: "nfm",
  bandwidthHz: 12_500,
  squelchDbfs: null,
  deemphasis: null,
  offsetHz: 0,
  status: "active",
  ...over,
});

function setup(v: Vfo | null, over: Partial<Parameters<typeof ReceiverFrequency>[0]> = {}) {
  const fns = { onStep: vi.fn(), onVfo: vi.fn(), onCenter: vi.fn() };
  render(
    <ReceiverFrequency
      vfo={v}
      step="auto"
      centerHz={101_000_000}
      lock={<span>lock-slot</span>}
      {...fns}
      {...over}
    />,
  );
  return fns;
}

it("with a VFO the big readout tunes that VFO", () => {
  const fns = setup(vfo());
  const group = screen.getByRole("group", { name: "VFO frequency" });
  fireEvent.keyDown(within(group).getByRole("spinbutton", { name: "100 kHz digit" }), {
    key: "ArrowUp",
  });
  expect(fns.onVfo).toHaveBeenCalledWith(2, { freqHz: 100_100_000 });
  expect(fns.onCenter).not.toHaveBeenCalled();
});

it("nudges the VFO by the tuning step (auto = mode default)", () => {
  const fns = setup(vfo());
  fireEvent.click(screen.getByRole("button", { name: "Up one step" }));
  expect(fns.onVfo).toHaveBeenLastCalledWith(2, { freqHz: 100_012_500 });
  fireEvent.click(screen.getByRole("button", { name: "Down one step" }));
  expect(fns.onVfo).toHaveBeenLastCalledWith(2, { freqHz: 99_987_500 });
});

it("changes the tuning step", () => {
  const fns = setup(vfo());
  fireEvent.change(screen.getByRole("combobox", { name: "Tuning step" }), {
    target: { value: "1000" },
  });
  expect(fns.onStep).toHaveBeenCalledWith(1000);
});

it("keeps the centre as a labelled secondary control next to the lock", () => {
  const fns = setup(vfo());
  const centre = screen.getByRole("group", { name: "Device centre" });
  expect(within(centre).getByText("Centre")).toBeInTheDocument();
  expect(within(centre).getByText("lock-slot")).toBeInTheDocument();
  fireEvent.keyDown(within(centre).getByRole("spinbutton", { name: "1 MHz digit" }), {
    key: "ArrowUp",
  });
  expect(fns.onCenter).toHaveBeenCalledWith(102_000_000);
  expect(fns.onVfo).not.toHaveBeenCalled();
});

it("typed entry on the VFO readout goes to the VFO", () => {
  const fns = setup(vfo());
  const group = screen.getByRole("group", { name: "VFO frequency" });
  fireEvent.keyDown(within(group).getByRole("spinbutton", { name: "1 kHz digit" }), {
    key: "Enter",
  });
  const input = screen.getByRole("textbox");
  fireEvent.change(input, { target: { value: "101.5" } });
  fireEvent.keyDown(input, { key: "Enter" });
  expect(fns.onVfo).toHaveBeenCalledWith(2, { freqHz: 101_500_000 });
});

it("flags a paused VFO", () => {
  setup(vfo({ status: "paused" }));
  expect(screen.getByText("paused")).toBeInTheDocument();
});

it("without a VFO the big readout tunes the centre and says so", () => {
  const fns = setup(null);
  expect(screen.queryByRole("group", { name: "VFO frequency" })).toBeNull();
  expect(screen.getAllByRole("group", { name: "Centre frequency" })).toHaveLength(1);
  expect(screen.getByText("Centre")).toBeInTheDocument();
  expect(screen.getByText("lock-slot")).toBeInTheDocument();
  fireEvent.keyDown(screen.getByRole("spinbutton", { name: "100 kHz digit" }), { key: "ArrowUp" });
  expect(fns.onCenter).toHaveBeenCalledWith(101_100_000);
  expect(fns.onVfo).not.toHaveBeenCalled();
});

it("typed entry goes to the centre when there is no VFO", () => {
  const fns = setup(null, { centerHz: 100_000_000 });
  fireEvent.keyDown(screen.getByRole("spinbutton", { name: "1 kHz digit" }), { key: "Enter" });
  const input = screen.getByRole("textbox");
  fireEvent.change(input, { target: { value: "101.5" } });
  fireEvent.keyDown(input, { key: "Enter" });
  expect(fns.onCenter).toHaveBeenCalledWith(101_500_000);
});
