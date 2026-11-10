import { describe, expect, it, vi } from "vitest";
import { ScanControls } from "./controls";
import {
  DEFAULT_SETTINGS,
  parseScannerState,
  rangeCount,
  sourceProblem,
  startBody,
  statusLabel,
  stoppedText,
  type ScannerState,
} from "./model";
import { parsePrefs } from "./prefs";
import { shortcutFor } from "../shell/shortcuts";

const holding = {
  state: "holding",
  phase: "signal",
  vfo: 1,
  pass: 3,
  window: { index: 2, count: 4, center_hz: 145000000, low_hz: 144000000, high_hz: 146000000 },
  current: {
    freq_hz: 145500000,
    mode: "nfm",
    bandwidth_hz: 12500,
    label: "Calling",
    level_dbfs: -30.2,
    margin_db: 41.5,
  },
  config: { kind: "list", name: "Test", count: 5, resume: "hang", hang_ms: 2000 },
  lockouts: [145600000],
  hits: [{ ts_ms: 5, freq_hz: 145500000, mode: "nfm", margin_db: 41.5 }, { bad: true }],
};

describe("parseScannerState", () => {
  it("reads a holding scanner", () => {
    const s = parseScannerState(holding);
    expect(s?.state).toBe("holding");
    expect(s?.current?.freqHz).toBe(145500000);
    expect(s?.current?.level?.marginDb).toBe(41.5);
    expect(s?.window?.count).toBe(4);
    expect(s?.lockouts).toEqual([145600000]);
    expect(s?.hits).toHaveLength(1);
    expect(s?.config?.name).toBe("Test");
    expect(statusLabel(s)).toEqual({ text: "Signal", tone: "success" });
  });

  it("rejects what it cannot read", () => {
    expect(parseScannerState(undefined)).toBeNull();
    expect(parseScannerState({ state: "dancing" })).toBeNull();
    expect(statusLabel(null).text).toBe("Idle");
  });

  it("names why a scan ended", () => {
    expect(stoppedText("claimed")).toMatch(/plugin/);
    expect(stoppedText("stopped")).toBeNull();
  });
});

describe("start request", () => {
  it("builds a range body", () => {
    const body = startBody(
      { kind: "range", startHz: 144000000, stopHz: 146000000, stepHz: 12500, mode: null },
      { ...DEFAULT_SETTINGS, priorityHz: 145500000 },
      2,
      [145600000],
    );
    expect(body).toMatchObject({
      v: 1,
      vfo: 2,
      threshold_db: 10,
      resume: "hang",
      lockout_hz: [145600000],
      priority: { freq_hz: 145500000, interval_ms: 5000 },
      source: { kind: "range", start_hz: 144000000, stop_hz: 146000000, step_hz: 12500 },
    });
  });

  it("builds a list body and checks sources", () => {
    const list = {
      kind: "list" as const,
      name: "PMR",
      channels: [{ freqHz: 446006250, mode: null, bandwidthHz: null, label: "PMR 1" }],
    };
    expect(startBody(list, DEFAULT_SETTINGS, null).source).toEqual({
      kind: "list",
      name: "PMR",
      channels: [{ freq_hz: 446006250, label: "PMR 1" }],
    });
    expect(sourceProblem(list)).toBeNull();
    expect(sourceProblem({ ...list, channels: [] })).toMatch(/no channels/);
    expect(rangeCount(144e6, 146e6, 12500)).toBe(161);
    expect(
      sourceProblem({ kind: "range", startHz: 1e6, stopHz: 900e6, stepHz: 100, mode: null }),
    ).toMatch(/too many/);
  });
});

describe("prefs", () => {
  it("clamps and falls back", () => {
    const p = parsePrefs({ source: "plan", startMhz: -5, settings: { dwellMs: 1, resume: "x" } });
    expect(p.source).toBe("plan");
    expect(p.startMhz).toBe(0.001);
    expect(p.settings.dwellMs).toBe(20);
    expect(p.settings.resume).toBe("hang");
  });
});

describe("shortcuts and controls", () => {
  it("maps the scanner keys", () => {
    const key = (k: string) => ({
      key: k,
      shiftKey: false,
      ctrlKey: false,
      altKey: false,
      metaKey: false,
    });
    expect(shortcutFor(key("g"))).toEqual({ type: "scan", command: "toggle" });
    expect(shortcutFor(key("p"))).toEqual({ type: "scan", command: "pause" });
    expect(shortcutFor(key("k"))).toEqual({ type: "scan", command: "skip" });
    expect(shortcutFor(key("l"))).toEqual({ type: "scan", command: "lockout" });
  });

  it("starts, stops, pauses and locks out through the api", async () => {
    const send = vi.fn().mockResolvedValue(undefined);
    const controls = new ScanControls("dev0", () => 3, { send });
    expect(controls.toggle()).toBe("setup");
    await controls.start({ v: 1, source: { kind: "range" } });
    expect(send).toHaveBeenLastCalledWith("dev0", "start", {
      v: 1,
      source: { kind: "range" },
      vfo: 3,
    });
    expect(controls.hasLast()).toBe(true);
    const running = parseScannerState(holding) as ScannerState;
    controls.ingest(running);
    expect(controls.togglePause()).toBe(true);
    expect(send).toHaveBeenLastCalledWith("dev0", "pause", undefined);
    expect(controls.lockoutHeld()).toBe(true);
    expect(send).toHaveBeenLastCalledWith("dev0", "lockout", { v: 1 });
    expect(controls.toggle()).toBe("stopped");
    controls.ingest(null);
    expect(controls.skipNow()).toBe(false);
    expect(controls.toggle()).toBe("started");
  });
});
