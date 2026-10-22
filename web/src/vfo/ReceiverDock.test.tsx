import { fireEvent, render, screen } from "@testing-library/react";
import { expect, it, vi } from "vitest";
import type { AudioController, AudioSnapshot } from "../audio/controller";
import type { Vfo } from "./api";
import type { VfoSnapshot } from "./controller";
import { ReceiverDock } from "./ReceiverDock";

const vfo = (id: number, over: Partial<Vfo> = {}): Vfo => ({
  id,
  freqHz: 100e6 + id * 1e5,
  mode: "nfm",
  bandwidthHz: 12_500,
  squelchDbfs: null,
  deemphasis: null,
  offsetHz: 0,
  status: "active",
  ...over,
});

const audio = {
  getMeter: () => null,
  setVolume: vi.fn(),
  setMuted: vi.fn(),
  enable: vi.fn(),
  disable: vi.fn(),
} as unknown as AudioController;

const audioSnap = {
  levels: { 2: { volume: 0.5, muted: false } },
  enabled: false,
  starting: false,
  unsupported: null,
} as unknown as AudioSnapshot;

function setup(vfos: Vfo[], selectedId: number | null, loaded = true) {
  const snap: VfoSnapshot = { vfos, selectedId, busy: false, error: null, loaded };
  const fns = {
    onStep: vi.fn(),
    onCreate: vi.fn(),
    onSelect: vi.fn(),
    onUpdate: vi.fn(),
    onRemove: vi.fn(),
  };
  render(<ReceiverDock vfos={snap} audio={audio} audioSnap={audioSnap} step="auto" {...fns} />);
  return fns;
}

it("shows the active VFO: chips, frequency, mode, squelch and volume", () => {
  setup([vfo(1), vfo(2, { mode: "am", bandwidthHz: 10_000 })], 2);
  expect(screen.getByRole("button", { name: /^VFO 1 NFM/ })).toHaveAttribute(
    "aria-pressed",
    "false",
  );
  expect(screen.getByRole("button", { name: /^VFO 2 AM/ })).toHaveAttribute("aria-pressed", "true");
  expect(screen.getByRole("group", { name: "VFO frequency" })).toBeInTheDocument();
  expect(screen.getByRole("radio", { name: "AM" })).toHaveAttribute("aria-checked", "true");
  expect(screen.getByRole("slider", { name: "Volume" })).toHaveValue("50");
  expect(screen.getByRole("slider", { name: "Squelch" })).toBeInTheDocument();
});

it("a mode change updates the active VFO and resets its bandwidth", () => {
  const fns = setup([vfo(1), vfo(2)], 2);
  fireEvent.click(screen.getByRole("radio", { name: "WFM" }));
  expect(fns.onUpdate).toHaveBeenCalledWith(2, { mode: "wfm", bandwidthHz: null });
});

it("nudges by the tuning step and tunes the active VFO from the readout", () => {
  const fns = setup([vfo(1)], 1);
  fireEvent.click(screen.getByRole("button", { name: "Up one step" }));
  expect(fns.onUpdate).toHaveBeenCalledWith(1, { freqHz: 100_112_500 });
  fireEvent.keyDown(screen.getByRole("spinbutton", { name: "10 kHz digit" }), { key: "ArrowUp" });
  expect(fns.onUpdate).toHaveBeenCalledWith(1, { freqHz: 100_110_000 });
});

it("selects a VFO from its chip and adds a new one", () => {
  const fns = setup([vfo(1), vfo(2)], 1);
  fireEvent.click(screen.getByRole("button", { name: /^VFO 2/ }));
  expect(fns.onSelect).toHaveBeenCalledWith(2);
  fireEvent.click(screen.getByRole("button", { name: "Add VFO" }));
  expect(fns.onCreate).toHaveBeenCalled();
});

it("marks a paused active VFO", () => {
  setup([vfo(1, { status: "paused" })], 1);
  expect(screen.getByText("paused")).toBeInTheDocument();
});

it("shows an empty state with an Add VFO button when there are no VFOs", () => {
  const fns = setup([], null);
  expect(screen.queryByRole("radiogroup", { name: "Mode" })).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "Add VFO" }));
  expect(fns.onCreate).toHaveBeenCalled();
});
