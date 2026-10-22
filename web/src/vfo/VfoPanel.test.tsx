import { render, screen, within } from "@testing-library/react";
import { expect, it, vi } from "vitest";
import type { AudioController, AudioSnapshot } from "../audio/controller";
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

function renderPanel(vfos: Vfo[], locked: boolean) {
  const snap: VfoSnapshot = { vfos, selectedId: null, busy: false, error: null, loaded: true };
  render(
    <VfoPanel
      vfos={snap}
      audio={{} as AudioController}
      audioSnap={{ levels: {} } as unknown as AudioSnapshot}
      step="auto"
      onStep={vi.fn()}
      locked={locked}
      onCreate={vi.fn()}
      onSelect={vi.fn()}
      onUpdate={vi.fn()}
      onRemove={vi.fn()}
    />,
  );
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
