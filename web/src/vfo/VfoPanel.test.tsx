import { fireEvent, render, screen, within } from "@testing-library/react";
import { expect, it, vi } from "vitest";
import type { Vfo } from "./api";
import type { VfoSnapshot } from "./controller";
import { VfoPanel } from "./VfoPanel";

const vfo = (id: number, status: Vfo["status"]): Vfo => ({
  id,
  freqHz: 100e6 + id * 1e5,
  mode: "nfm",
  bandwidthHz: 12_500,
  squelchDbfs: null,
  deemphasis: null,
  offsetHz: 0,
  status,
});

function renderPanel(vfos: Vfo[], locked: boolean, selectedId: number | null = null) {
  const snap: VfoSnapshot = { vfos, selectedId, busy: false, error: null, loaded: true };
  const fns = { onCreate: vi.fn(), onSelect: vi.fn(), onRemove: vi.fn(), onListen: vi.fn() };
  render(<VfoPanel vfos={snap} locked={locked} isListening={(id) => id !== 2} {...fns} />);
  return fns;
}

it("marks a paused VFO in the list and explains the tuning lock", () => {
  renderPanel([vfo(1, "active"), vfo(2, "paused")], true);
  const rows = within(screen.getByRole("list", { name: "VFO list" })).getAllByRole("listitem");
  expect(rows[0]).not.toHaveTextContent("paused");
  expect(rows[1]).toHaveTextContent("paused");
  expect(screen.getByText("paused")).toHaveAttribute(
    "title",
    "Tuning locked — VFO paused outside the window",
  );
  expect(screen.getByRole("status")).toHaveTextContent(
    "Tuning locked — VFO paused outside the window",
  );
});

it("explains a paused VFO differently without a lock", () => {
  renderPanel([vfo(1, "paused")], false);
  expect(screen.getByRole("status")).toHaveTextContent("outside the usable window");
});

it("shows no paused hint when every VFO is active", () => {
  renderPanel([vfo(1, "active")], true);
  expect(screen.queryByText("paused")).toBeNull();
  expect(screen.queryByRole("status")).toBeNull();
});

it("selects and deletes from the compact list, with no editor in the sidebar", () => {
  const fns = renderPanel([vfo(1, "active"), vfo(2, "active")], false, 1);
  const rows = within(screen.getByRole("list", { name: "VFO list" })).getAllByRole("listitem");
  fireEvent.click(within(rows[1]!).getAllByRole("button")[0]!);
  expect(fns.onSelect).toHaveBeenCalledWith(2);
  fireEvent.click(screen.getByRole("button", { name: "Delete VFO 1" }));
  expect(fns.onRemove).toHaveBeenCalledWith(1);
  expect(screen.queryByRole("radiogroup", { name: "Mode" })).toBeNull();
});

it("toggles listening per VFO from the list", () => {
  const fns = renderPanel([vfo(1, "active"), vfo(2, "active")], false, 1);
  const on = screen.getByRole("button", { name: "Listen to VFO 1" });
  const off = screen.getByRole("button", { name: "Listen to VFO 2" });
  expect(on).toHaveAttribute("aria-pressed", "true");
  expect(off).toHaveAttribute("aria-pressed", "false");
  fireEvent.click(off);
  expect(fns.onListen).toHaveBeenCalledWith(2, true);
  fireEvent.click(on);
  expect(fns.onListen).toHaveBeenCalledWith(1, false);
});

it("explains how to add the first VFO", () => {
  renderPanel([], false);
  expect(screen.getByText("Click the spectrum or press + to add a VFO")).toBeInTheDocument();
});
