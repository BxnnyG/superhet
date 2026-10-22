import { expect, it, vi } from "vitest";
import type { TuningController } from "../spectrum/tuning";
import { spectrumTuningView } from "./spectrumTuning";

function setup(vfoCount: number, loaded = true) {
  const tuning = {
    request: vi.fn(),
    refresh: vi.fn(),
    getSnapshot: vi.fn(() => "snapshot"),
  };
  const create = vi.fn();
  const view = spectrumTuningView(
    tuning as unknown as TuningController,
    { getSnapshot: () => ({ vfos: Array(vfoCount).fill({}), loaded }) } as never,
    create,
  );
  return { tuning, create, view };
}

it("creates a VFO instead of moving the centre while there is none", () => {
  const { view, tuning, create } = setup(0);
  view.request({ centerHz: 100_500_000 });
  expect(create).toHaveBeenCalledWith(100_500_000);
  expect(tuning.request).not.toHaveBeenCalled();
});

it("waits for the VFO list before it does anything", () => {
  const { view, tuning, create } = setup(0, false);
  view.request({ centerHz: 100_500_000 });
  expect(create).not.toHaveBeenCalled();
  expect(tuning.request).not.toHaveBeenCalled();
});

it("passes tune requests through once a VFO exists", () => {
  const { view, tuning, create } = setup(1);
  view.request({ centerHz: 100_500_000 });
  expect(tuning.request).toHaveBeenCalledWith({ centerHz: 100_500_000 });
  expect(create).not.toHaveBeenCalled();
});

it("passes requests without a centre through and forwards other members", () => {
  const { view, tuning } = setup(0);
  view.request({ sampleRate: 2_048_000 });
  expect(tuning.request).toHaveBeenCalledWith({ sampleRate: 2_048_000 });
  void view.refresh();
  expect(tuning.refresh).toHaveBeenCalled();
  expect(view.getSnapshot()).toBe("snapshot");
});
