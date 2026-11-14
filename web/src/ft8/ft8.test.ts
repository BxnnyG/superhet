import { describe, expect, it } from "vitest";
import { parseEvent, type DecoderEvent } from "../decoders/events";
import { describeEvent } from "../decoders/render";
import {
  FT4_PRESETS,
  FT8_PLUGIN_ID,
  FT8_PRESETS,
  WSPR_PRESETS,
  gridCentre,
  presetAt,
  presetsOf,
} from "./model";

// The data objects are what the adapter prints (crates/superhet-plugin-adapters/src/ft8/lines.rs).
const make = (kind: string, data: Record<string, unknown>): DecoderEvent => {
  const e = parseEvent({
    id: 1,
    ts_ms: 1_794_657_615_000,
    device: "dev0",
    vfo: 1,
    freq_hz: 14_074_000,
    mode: null,
    kind,
    source: FT8_PLUGIN_ID,
    value: String(data.summary ?? ""),
    meta: { instance: "i-1", ts: null, data },
  });
  if (!e) throw new Error("fixture");
  return e;
};

describe("Maidenhead locators", () => {
  it("returns the centre of a square", () => {
    expect(gridCentre("JO62")).toEqual([52.5, 13]);
    expect(gridCentre("FN42")).toEqual([42.5, -71]);
    const [lat, lon] = gridCentre("JO62qm") ?? [0, 0];
    expect(lat).toBeCloseTo(52.5208, 3);
    expect(lon).toBeCloseTo(13.375, 3);
  });

  it("refuses anything else", () => {
    for (const bad of ["", "JO6", "ZZ99", "JO62ZZ", "hello", "SO62"]) {
      expect(gridCentre(bad), bad).toBeNull();
    }
  });
});

describe("band presets", () => {
  it("lists the usual dial frequencies per mode", () => {
    expect(FT8_PRESETS.find((p) => p.band === "20 m")?.dialHz).toBe(14_074_000);
    expect(FT4_PRESETS.find((p) => p.band === "40 m")?.dialHz).toBe(7_047_500);
    expect(WSPR_PRESETS.find((p) => p.band === "20 m")?.dialHz).toBe(14_095_600);
    expect(presetsOf("wspr")).toBe(WSPR_PRESETS);
    for (const list of [FT8_PRESETS, FT4_PRESETS, WSPR_PRESETS]) {
      const hz = list.map((p) => p.dialHz);
      expect(new Set(hz).size).toBe(hz.length);
      expect([...hz].sort((a, b) => a - b)).toEqual(hz);
    }
  });

  it("finds the band a receiver sits on", () => {
    expect(presetAt(FT8_PRESETS, 14_074_000)?.band).toBe("20 m");
    expect(presetAt(FT8_PRESETS, 14_074_400)?.band).toBe("20 m");
    expect(presetAt(FT8_PRESETS, 14_080_000)).toBeNull();
    expect(presetAt(FT8_PRESETS, 100_000_000)).toBeNull();
  });
});

describe("ft8.message / wspr.spot events", () => {
  it("shows a CQ call with its locator and a map link", () => {
    const v = describeEvent(
      make("ft8.message", {
        mode: "ft8",
        time: "2026-11-14T12:00:15Z",
        snr: -12,
        dt: 0.3,
        freq_hz: 1234,
        dial_hz: 14_074_000,
        rf_hz: 14_075_234,
        message: "CQ DL1ABC JO62",
        cq: true,
        callsign: "DL1ABC",
        grid: "JO62",
        summary: "FT8 12:00:15 -12 dB 1234 Hz CQ DL1ABC JO62",
      }),
    );
    expect(v.label).toBe("FT8");
    expect(v.headline).toBe("DL1ABC");
    expect(v.tags).toEqual(["-12 dB", "dt +0.3 s", "14.075234 MHz"]);
    expect(v.detail).toBe("CQ DL1ABC JO62");
    expect(v.link?.href).toContain("mlat=52.50000&mlon=13.00000");
    expect(v.fields.find((f) => f.label === "Locator")?.value).toBe("JO62");
  });

  it("shows who a directed message goes to", () => {
    const v = describeEvent(
      make("ft8.message", {
        mode: "ft4",
        time: "2026-11-14T12:00:07.500Z",
        snr: 3,
        dt: -0.1,
        freq_hz: 800,
        message: "DL1ABC K1ABC R-05",
        cq: false,
        callsign: "K1ABC",
        to: "DL1ABC",
        summary: "x",
      }),
    );
    expect(v.label).toBe("FT4");
    expect(v.headline).toBe("K1ABC");
    expect(v.detail).toBe("DL1ABC ← DL1ABC K1ABC R-05");
    expect(v.tags).toEqual(["+3 dB", "dt -0.1 s", "800 Hz"]);
    expect(v.link).toBeNull();
  });

  it("falls back to the message for free text", () => {
    const v = describeEvent(
      make("ft8.message", { mode: "ft8", message: "TNX 73", cq: false, summary: "x" }),
    );
    expect(v.headline).toBe("TNX");
    expect(v.detail).toBe("TNX 73");
  });

  it("shows a WSPR spot", () => {
    const v = describeEvent(
      make("wspr.spot", {
        mode: "wspr",
        time: "2026-11-14T12:00:00Z",
        snr: -21,
        dt: 0.8,
        freq_hz: 1452,
        drift: -1,
        callsign: "K1ABC",
        grid: "FN42",
        power_dbm: 37,
        message: "K1ABC FN42 37",
        summary: "x",
      }),
    );
    expect(v.label).toBe("WSPR");
    expect(v.headline).toBe("K1ABC");
    expect(v.detail).toBe("FN42 · 37 dBm · drift -1 Hz/min");
    expect(v.fields.find((f) => f.label === "Power")?.value).toBe("37 dBm");
    expect(v.link?.label).toBe("Open locator on map");
  });
});
