import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { fake, vfo } from "../shell/fixtures";
import { TuningStrip } from "../shell/TuningStrip";
import type { Receiver } from "../shell/useReceiver";
import { ChannelsPanel, LONG_PRESS_MS, planSpan } from "./ChannelsPanel";
import { builtinPlans } from "./catalog";
import { buildIndex, matchChannel } from "./lookup";
import { DEFAULT_PREFS } from "./prefs";
import { ChannelStore } from "./store";

beforeEach(() => vi.clearAllMocks());
afterEach(() => vi.useRealTimers());

function setup(region: string | null = "DE", vfoFreq = 100e6) {
  const { r, fns } = fake([vfo(1, { freqHz: vfoFreq })], 1);
  const store = new ChannelStore(
    {
      list: async () => ({ plans: [], skipped: 0 }),
      create: async () => undefined,
      update: async () => undefined,
      remove: async () => undefined,
    },
    { ...DEFAULT_PREFS, region },
    null,
    () => undefined,
  );
  const tuneChannel = vi.fn();
  const stepChannels = vi.fn(() => false);
  const snap = store.getSnapshot();
  Object.assign(r, {
    channels: store,
    channelSnap: snap,
    channelMatch: matchChannel(buildIndex(snap.plans), vfoFreq, { region, rasters: {} }),
    tuneChannel,
    stepChannels,
  });
  return { r: r as Receiver, fns, store, tuneChannel, stepChannels };
}

it("lists the services of the region first and keeps the others reachable", () => {
  const { r } = setup("DE");
  render(<ChannelsPanel r={r} />);
  const mine = screen.getByRole("region", { name: "For Germany" });
  expect(within(mine).getByText("PMR446")).toBeInTheDocument();
  expect(within(mine).getByText("Freenet")).toBeInTheDocument();
  expect(within(mine).getByText("Marine VHF (ship side)")).toBeInTheDocument();
  expect(within(mine).queryByText("Air band (VHF voice)")).not.toBeNull(); // worldwide
  const other = screen.queryByRole("region", { name: "Other regions" });
  expect(other).toBeNull(); // everything built in applies in Germany

  const us = setup("US");
  const { unmount } = render(<ChannelsPanel r={us.r} />);
  const others = screen.getAllByRole("region", { name: "Other regions" }).at(-1)!;
  expect(within(others).getByText("Freenet")).toBeInTheDocument();
  expect(within(others).getByText("PMR446")).toBeInTheDocument();
  unmount();
});

it("shows licence, frequency span and the HF warning of CB", () => {
  const { r } = setup("DE");
  render(<ChannelsPanel r={r} />);
  const card = screen.getByText("CB radio (40 channels)").closest("button")!;
  expect(within(card).getByText("Licence-free")).toBeInTheDocument();
  expect(within(card).getByText("HF")).toBeInTheDocument();
  fireEvent.click(card);
  expect(screen.getByText(/for good reception/)).toBeInTheDocument();
  expect(planSpan(builtinPlans().find((p) => p.id === "pmr446")!)).toBe(
    "446.006 250 - 446.193 750 MHz",
  );
});

it("a channel tap tunes the active VFO, Alt-click makes a new one", () => {
  const { r, tuneChannel } = setup("DE");
  render(<ChannelsPanel r={r} />);
  fireEvent.click(screen.getByText("PMR446").closest("button")!);
  const grid = screen.getByRole("list", { name: "PMR446 channels" });
  expect(within(grid).getAllByRole("button")).toHaveLength(16);
  fireEvent.click(within(grid).getByRole("button", { name: /^PMR 3,/ }));
  expect(tuneChannel).toHaveBeenLastCalledWith(
    expect.objectContaining({ id: "pmr446" }),
    expect.objectContaining({ number: "3", freqHz: 446_031_250 }),
    false,
  );
  fireEvent.click(within(grid).getByRole("button", { name: /^PMR 4,/ }), { altKey: true });
  expect(tuneChannel).toHaveBeenLastCalledWith(
    expect.anything(),
    expect.objectContaining({ number: "4" }),
    true,
  );
});

it("a long press on touch opens a new VFO, once", () => {
  vi.useFakeTimers();
  const { r, tuneChannel } = setup("DE");
  render(<ChannelsPanel r={r} />);
  fireEvent.click(screen.getByText("PMR446").closest("button")!);
  const button = screen.getByRole("button", { name: /^PMR 5,/ });
  fireEvent.pointerDown(button, { pointerType: "touch" });
  act(() => void vi.advanceTimersByTime(LONG_PRESS_MS + 10));
  fireEvent.pointerUp(button, { pointerType: "touch" });
  fireEvent.click(button);
  expect(tuneChannel).toHaveBeenCalledTimes(1);
  expect(tuneChannel.mock.calls[0]?.[2]).toBe(true);
  // A short tap is a plain tune.
  fireEvent.pointerDown(button, { pointerType: "touch" });
  fireEvent.pointerUp(button, { pointerType: "touch" });
  fireEvent.click(button);
  expect(tuneChannel).toHaveBeenCalledTimes(2);
  expect(tuneChannel.mock.calls[1]?.[2]).toBe(false);
});

it("highlights the channel the VFO sits on and filters long channel lists", () => {
  const { r } = setup("DE", 156_800_000);
  render(<ChannelsPanel r={r} />);
  fireEvent.click(screen.getByText("Marine VHF (ship side)").closest("button")!);
  expect(screen.getByRole("button", { name: /^Marine 16,/ })).toHaveAttribute(
    "aria-pressed",
    "true",
  );
  expect(screen.getByRole("button", { name: /^Marine 17,/ })).toHaveAttribute(
    "aria-pressed",
    "false",
  );
  fireEvent.change(screen.getByLabelText("Filter channels"), { target: { value: "distress" } });
  expect(screen.getAllByRole("button", { name: /^Marine/ })).toHaveLength(1);
  fireEvent.change(screen.getByLabelText("Filter channels"), { target: { value: "zzz" } });
  expect(screen.getByText("No such channel")).toBeInTheDocument();
});

it("DAB: explains the plugin and the centre change", () => {
  const { r, tuneChannel } = setup("DE");
  render(<ChannelsPanel r={r} />);
  fireEvent.click(screen.getByText("DAB / DAB+ Band III").closest("button")!);
  expect(
    screen.getByText(/DAB decoding needs the DAB plugin \(coming in P2\)/),
  ).toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: /^DAB 5C,/ }), { altKey: true });
  // Alt never opens a VFO for a DAB block.
  expect(tuneChannel.mock.calls[0]?.[2]).toBe(false);
});

it("the air band shows its raster choice instead of a channel list", () => {
  const { r, store } = setup("DE");
  render(<ChannelsPanel r={r} />);
  fireEvent.click(screen.getByText("Air band (VHF voice)").closest("button")!);
  fireEvent.click(screen.getByRole("radio", { name: "8.33 kHz" }));
  expect(store.getSnapshot().prefs.rasters).toEqual({ airband: "8.33k" });
  expect(screen.getAllByRole("button", { name: /^Air / })).toHaveLength(1); // the guard channel
});

it("region and stepping settings go to the store; search narrows the cards", () => {
  const { r, store } = setup("DE");
  render(<ChannelsPanel r={r} />);
  fireEvent.change(screen.getByRole("combobox", { name: "Region" }), { target: { value: "US" } });
  expect(store.getSnapshot().region).toBe("US");
  fireEvent.click(screen.getByRole("switch", { name: "Step by channel" }));
  expect(store.getSnapshot().prefs.stepByChannel).toBe(false);
  fireEvent.change(screen.getByLabelText("Find a service"), { target: { value: "marine" } });
  expect(screen.queryByText("PMR446")).toBeNull();
  expect(screen.getByText("Marine VHF (ship side)")).toBeInTheDocument();
  fireEvent.change(screen.getByLabelText("Find a service"), { target: { value: "qqq" } });
  expect(screen.getByText("No matching service")).toBeInTheDocument();
});

it("the strip chip names the channel, toggles channel stepping and the arrows ask for channels", () => {
  const { r, fns, store, stepChannels } = setup("DE", 446_031_250);
  const { rerender } = render(<TuningStrip r={r} />);
  const chip = screen.getByRole("button", { name: /Channel PMR 3/ });
  expect(chip).toHaveTextContent("PMR 3");
  expect(chip).toHaveAttribute("aria-pressed", "true");
  // The channel step handled it: no frequency step.
  stepChannels.mockReturnValueOnce(true);
  fireEvent.click(screen.getByRole("button", { name: "Up one step" }));
  expect(stepChannels).toHaveBeenCalledWith(1);
  expect(fns.update).not.toHaveBeenCalled();
  // Not handled (stepping off or no channel): the kHz step.
  fireEvent.click(screen.getByRole("button", { name: "Down one step" }));
  expect(stepChannels).toHaveBeenLastCalledWith(-1);
  expect(fns.update).toHaveBeenCalledTimes(1);
  fireEvent.click(chip);
  expect(store.getSnapshot().prefs.stepByChannel).toBe(false);
  rerender(<TuningStrip r={{ ...r, channelSnap: store.getSnapshot() } as Receiver} />);
  expect(screen.getByRole("button", { name: /Channel PMR 3/ })).toHaveAttribute(
    "aria-pressed",
    "false",
  );
});

it("the strip shows no chip off a channel", () => {
  const { r } = setup("DE", 100e6);
  render(<TuningStrip r={r} />);
  expect(screen.queryByRole("button", { name: /^Channel / })).toBeNull();
});
