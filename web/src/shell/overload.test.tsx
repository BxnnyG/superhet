import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { DeviceState } from "../spectrum/api";
import { TuningControls } from "../spectrum/TuningControls";
import type { TuningSnapshot } from "../spectrum/tuning";
import { overloadNotice } from "./overload";

const device = (over: Partial<DeviceState> = {}): DeviceState => ({
  driver: "rtlsdr",
  centerHz: 100_000_000,
  sampleRate: 2_048_000,
  gain: { mode: "manual", db: 28 },
  gainsDb: [0, 28, 49.6],
  ppm: 0,
  biasTee: false,
  tuningEpoch: 1,
  locks: [],
  window: { startHz: 99_000_000, endHz: 101_000_000 },
  ...over,
});

const snapshot = (state: DeviceState): TuningSnapshot => ({
  state,
  display: state,
  busy: false,
  lockBusy: false,
  error: null,
});

describe("overloadNotice", () => {
  it("is silent without an overload", () => {
    expect(overloadNotice(null)).toBeNull();
    expect(overloadNotice(device())).toBeNull();
    expect(overloadNotice(device({ overload: { active: false, clippedPercent: 0.2 } }))).toBeNull();
  });

  it("tells to lower a manual gain", () => {
    const n = overloadNotice(device({ overload: { active: true, clippedPercent: 7.34 } }));
    expect(n?.summary).toContain("7.3 %");
    expect(n?.action).toMatch(/^Lower the gain/);
  });

  it("points at automatic gain when it is on", () => {
    const n = overloadNotice(
      device({ gain: { mode: "auto", db: null }, overload: { active: true, clippedPercent: 31 } }),
    );
    expect(n?.summary).toContain("31 %");
    expect(n?.action).toMatch(/Automatic gain off/);
  });
});

describe("source settings", () => {
  it("shows the overload warning above the gain controls", () => {
    render(
      <TuningControls
        tuning={snapshot(device({ overload: { active: true, clippedPercent: 5 } }))}
        onTune={vi.fn()}
      />,
    );
    expect(screen.getByRole("alert")).toHaveTextContent(/ADC overload/);
    expect(screen.getByRole("switch", { name: "Automatic gain" })).toBeInTheDocument();
  });

  it("shows nothing when the front end is fine", () => {
    render(<TuningControls tuning={snapshot(device())} onTune={vi.fn()} />);
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });
});
