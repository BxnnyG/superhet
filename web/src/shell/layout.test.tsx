import { fireEvent, render, screen, within } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";
import { CentrePill } from "./CentrePill";
import { fake, user, vfo } from "./fixtures";
import { TuningStrip } from "./TuningStrip";
import { VfoTabs } from "./VfoTabs";

beforeEach(() => vi.clearAllMocks());

it("strip: one big frequency that tunes the selected VFO, plus step, nudge and bookmark", () => {
  const { r, fns } = fake([vfo(1), vfo(2)], 2);
  render(<TuningStrip r={r} />);
  // The only frequency readout on screen is the VFO's; the centre is a pill, not an input.
  expect(screen.getAllByRole("group", { name: /frequency$/ })).toHaveLength(1);
  expect(screen.getByRole("group", { name: "VFO frequency" })).toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: "Up one step" }));
  expect(fns.update).toHaveBeenCalledWith(2, { freqHz: 100_212_500 });
  fireEvent.click(screen.getByRole("button", { name: "Down one step" }));
  expect(fns.update).toHaveBeenLastCalledWith(2, { freqHz: 100_187_500 });
  fireEvent.click(screen.getByRole("button", { name: "Bookmark this frequency" }));
  expect(fns.startBookmark).toHaveBeenCalled();
  fireEvent.change(screen.getByRole("combobox", { name: "Tuning step" }), {
    target: { value: "25000" },
  });
  expect(fns.setStep).toHaveBeenCalledWith(25_000);
});

it("strip: mode, bandwidth and squelch edit the selected VFO", () => {
  const { r, fns } = fake([vfo(1), vfo(2)], 2);
  render(<TuningStrip r={r} />);
  fireEvent.click(screen.getByRole("radio", { name: "WFM" }));
  expect(fns.update).toHaveBeenCalledWith(2, { mode: "wfm", bandwidthHz: null });
  fireEvent.change(screen.getByRole("combobox", { name: "Bandwidth" }), {
    target: { value: "default" },
  });
  expect(fns.update).toHaveBeenLastCalledWith(2, { bandwidthHz: null });
  fireEvent.change(screen.getByRole("slider", { name: "Squelch" }), { target: { value: "-70" } });
  expect(fns.update).toHaveBeenLastCalledWith(2, { squelchDbfs: -70 });
});

it("strip: without a VFO it offers to create one instead of tuning the centre", () => {
  const { r, fns } = fake([], null);
  render(<TuningStrip r={r} />);
  expect(screen.queryByRole("spinbutton")).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "Create receiver" }));
  expect(fns.createVfoAt).toHaveBeenCalledWith(100e6);
});

it("strip: marks a paused VFO", () => {
  const { r } = fake([vfo(1, { status: "paused" })], 1);
  render(<TuningStrip r={r} />);
  expect(screen.getByText("paused")).toBeInTheDocument();
});

it("strip: says which plugin owns the device instead of a bare 'paused'", () => {
  const { r } = fake([vfo(1, { status: "paused", freqHz: 1_089_812_500 })], 1);
  Object.assign(r, {
    claim: { instanceId: "i-dab", pluginId: "org.superhet.decoder.dab", name: "DAB" },
  });
  render(<TuningStrip r={r} />);
  const badge = screen.getByText("DAB has the device");
  expect(screen.queryByText("paused")).toBeNull();
  // The tooltip names the device centre and where the VFO resumes.
  expect(badge.closest("[title]")).toHaveAttribute(
    "title",
    expect.stringContaining("keeps 1089.8125 MHz and resumes when DAB stops"),
  );
  expect(badge.closest("[title]")).toHaveAttribute(
    "title",
    expect.stringContaining("at 100.000 MHz"),
  );
});

it("tab row: a paused VFO's tooltip names the plugin that owns the device", () => {
  const { r } = fake([vfo(1, { status: "paused" })], 1);
  Object.assign(r, { claim: { instanceId: "i-a", pluginId: null, name: "A plugin" } });
  render(<VfoTabs r={r} />);
  expect(screen.getByRole("button", { name: /^VFO 1 .*\(paused\)/ })).toHaveAttribute(
    "title",
    expect.stringContaining("A plugin is using the whole device"),
  );
});

it("centre pill shows the centre and opens the source settings", () => {
  const { r, fns } = fake([vfo(1)], 1);
  render(<CentrePill r={r} />);
  const pill = screen.getByRole("button", { name: /Centre 100\.000/ });
  expect(screen.queryByRole("dialog", { name: "Source settings" })).toBeNull();
  fireEvent.click(pill);
  const dialog = screen.getByRole("dialog", { name: "Source settings" });
  expect(within(dialog).getByLabelText("Centre frequency")).toBeInTheDocument();
  expect(within(dialog).getByLabelText("Sample rate")).toBeInTheDocument();
  expect(within(dialog).getByRole("button", { name: "Lock tuning" })).toBeInTheDocument();
  fireEvent.click(within(dialog).getByRole("button", { name: "Lock tuning" }));
  expect(fns.toggleLock).toHaveBeenCalledWith(true);
  fireEvent.keyDown(window, { key: "Escape" });
  expect(screen.queryByRole("dialog", { name: "Source settings" })).toBeNull();
});

it("centre pill shows the lock and the centre input is disabled while locked", () => {
  const { r } = fake([vfo(1)], 1, [user]);
  render(<CentrePill r={r} />);
  fireEvent.click(screen.getByRole("button", { name: /Centre 100\.000, locked/ }));
  expect(screen.getByLabelText("Centre frequency")).toBeDisabled();
});

it("tab row: selects, closes and toggles listening per VFO, and adds a VFO", () => {
  const { r, fns } = fake([vfo(1), vfo(2)], 2);
  render(<VfoTabs r={r} />);
  expect(screen.getByRole("button", { name: /^VFO 1 NFM/ })).toHaveAttribute(
    "aria-pressed",
    "false",
  );
  expect(screen.getByRole("button", { name: /^VFO 2 NFM/ })).toHaveAttribute(
    "aria-pressed",
    "true",
  );
  fireEvent.click(screen.getByRole("button", { name: /^VFO 1 / }));
  expect(fns.select).toHaveBeenCalledWith(1);
  expect(fns.setListening).not.toHaveBeenCalled();
  const one = screen.getByRole("button", { name: "Listen to VFO 1" });
  const two = screen.getByRole("button", { name: "Listen to VFO 2" });
  expect(one).toHaveAttribute("aria-pressed", "false");
  expect(two).toHaveAttribute("aria-pressed", "true");
  fireEvent.click(one);
  expect(fns.setListening).toHaveBeenCalledWith(1, true);
  fireEvent.click(screen.getByRole("button", { name: "Delete VFO 2" }));
  expect(fns.remove).toHaveBeenCalledWith(2);
  fireEvent.click(screen.getByRole("button", { name: "Add VFO" }));
  expect(fns.createVfoAt).toHaveBeenCalledWith(100e6);
});

it("tab row: no fixed VFO limit, the server enforces quotas", () => {
  const vfos = Array.from({ length: 12 }, (_, i) => vfo(i + 1));
  const { r } = fake(vfos, 1);
  render(<VfoTabs r={r} />);
  expect(screen.getByRole("button", { name: "Add VFO" })).toBeEnabled();
});
