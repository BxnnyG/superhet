import { act, render, renderHook, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { parseDeviceState, type DeviceState } from "../spectrum/api";
import { TuningControls } from "../spectrum/TuningControls";
import type { TuningSnapshot } from "../spectrum/tuning";
import { droppedNotice, RECENT_MS, useRecentDrops } from "./dropped";

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

describe("droppedNotice", () => {
  it("is hidden while nothing was dropped", () => {
    expect(droppedNotice(null)).toBeNull();
    expect(droppedNotice(device())).toBeNull();
    expect(droppedNotice(device({ droppedBlocks: 0 }))).toBeNull();
  });

  it("reports the total", () => {
    expect(droppedNotice(device({ droppedBlocks: 1 }))?.summary).toMatch(/^1 IQ block was/);
    expect(droppedNotice(device({ droppedBlocks: 1234 }))?.summary).toMatch(
      /^1,234 IQ blocks were/,
    );
  });
});

describe("state parsing", () => {
  const raw = {
    driver: "file",
    center_hz: 1e8,
    sample_rate: 2_048_000,
    gain: null,
    gains_db: [],
  };

  it("reads dropped_blocks and tolerates its absence", () => {
    expect(parseDeviceState({ ...raw, dropped_blocks: 12 }).droppedBlocks).toBe(12);
    expect(parseDeviceState(raw).droppedBlocks).toBeUndefined();
    expect(parseDeviceState({ ...raw, dropped_blocks: "x" }).droppedBlocks).toBeUndefined();
  });
});

describe("source settings", () => {
  it("shows the dropped-block notice only after drops", () => {
    const { rerender } = render(<TuningControls tuning={snapshot(device())} onTune={vi.fn()} />);
    expect(screen.queryByTestId("dropped-notice")).not.toBeInTheDocument();
    rerender(<TuningControls tuning={snapshot(device({ droppedBlocks: 3 }))} onTune={vi.fn()} />);
    expect(screen.getByTestId("dropped-notice")).toHaveTextContent(/3 IQ blocks were dropped/);
  });
});

describe("useRecentDrops", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it("ignores history, lights up on growth and fades after a minute", () => {
    const { result, rerender } = renderHook(({ total }) => useRecentDrops(total), {
      initialProps: { total: 5 as number | undefined },
    });
    expect(result.current).toBe(false);
    rerender({ total: 5 });
    expect(result.current).toBe(false);
    rerender({ total: 9 });
    expect(result.current).toBe(true);
    act(() => {
      vi.advanceTimersByTime(RECENT_MS + 1);
    });
    expect(result.current).toBe(false);
  });
});
