import { describe, expect, it } from "vitest";
import { parseTuneCommand } from "./parse";

describe("parseTuneCommand", () => {
  it("reads a bare frequency as MHz", () => {
    expect(parseTuneCommand("433.92")).toEqual({ freqHz: 433_920_000 });
    expect(parseTuneCommand("  104.2 ")).toEqual({ freqHz: 104_200_000 });
  });

  it("reads units glued to or separated from the number", () => {
    expect(parseTuneCommand("145.5m")).toEqual({ freqHz: 145_500_000 });
    expect(parseTuneCommand("145.5 m")).toEqual({ freqHz: 145_500_000 });
    expect(parseTuneCommand("7100k")).toEqual({ freqHz: 7_100_000 });
    expect(parseTuneCommand("7100 kHz")).toEqual({ freqHz: 7_100_000 });
    expect(parseTuneCommand("145500000")).toEqual({ freqHz: 145_500_000 });
    expect(parseTuneCommand("2.4 GHz")).toEqual({ freqHz: 2_400_000_000 });
  });

  it("accepts a decimal comma", () => {
    expect(parseTuneCommand("100,1 wfm")).toEqual({ freqHz: 100_100_000, mode: "wfm" });
  });

  it("reads a mode, case-insensitively and with aliases", () => {
    expect(parseTuneCommand("7.1 lsb")).toEqual({ freqHz: 7_100_000, mode: "lsb" });
    expect(parseTuneCommand("14.2 USB")).toEqual({ freqHz: 14_200_000, mode: "usb" });
    expect(parseTuneCommand("118.1 am")).toEqual({ freqHz: 118_100_000, mode: "am" });
    expect(parseTuneCommand("7.03 cw")).toEqual({ freqHz: 7_030_000, mode: "cw" });
    expect(parseTuneCommand("145.5 fm")).toEqual({ freqHz: 145_500_000, mode: "nfm" });
  });

  it("reads a CTCSS tone and implies NFM", () => {
    expect(parseTuneCommand("446.00625 nfm ctcss 88.5")).toEqual({
      freqHz: 446_006_250,
      mode: "nfm",
      toneHz: 88.5,
    });
    expect(parseTuneCommand("446.00625 ctcss 88.5")).toEqual({
      freqHz: 446_006_250,
      mode: "nfm",
      toneHz: 88.5,
    });
    expect(parseTuneCommand("446.00625 tone 67")).toMatchObject({ toneHz: 67 });
    expect(parseTuneCommand("446.00625 ctcss88.5 am")).toMatchObject({ mode: "am", toneHz: 88.5 });
    expect(parseTuneCommand("446.00625 pl 88,5")).toMatchObject({ toneHz: 88.5 });
  });

  it("rejects what is not a clean command", () => {
    expect(parseTuneCommand("")).toBeNull();
    expect(parseTuneCommand("tetra")).toBeNull();
    expect(parseTuneCommand("new vfo")).toBeNull();
    expect(parseTuneCommand("0")).toBeNull();
    expect(parseTuneCommand("145.5 foo")).toBeNull();
    expect(parseTuneCommand("145.5 nfm wfm")).toBeNull();
    // Not a standard CTCSS tone, missing tone, tone with a mode that has none.
    expect(parseTuneCommand("446 ctcss 88.6")).toBeNull();
    expect(parseTuneCommand("446 ctcss")).toBeNull();
    expect(parseTuneCommand("100.1 wfm ctcss 88.5")).toBeNull();
    expect(parseTuneCommand("7.1 lsb tone 67")).toBeNull();
  });

  it("rejects frequencies beyond the tuner range", () => {
    expect(parseTuneCommand("99999 ghz")).toBeNull();
  });
});
