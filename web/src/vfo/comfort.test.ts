import { describe, expect, it } from "vitest";
import { createBody, parseVfo, patchBody } from "./api";
import {
  audioRange,
  clampNotch,
  comfortBody,
  comfortIsOff,
  comfortSupported,
  parseComfort,
  toggleComfort,
} from "./comfort";

describe("comfort helpers", () => {
  it("toggles blocks on at their start value and off again", () => {
    const on = toggleComfort(undefined, "nr");
    expect(on).toEqual({ nr: 0.5 });
    expect(toggleComfort(on, "notch").notch).toEqual({ freqHz: 1000, widthHz: 60 });
    expect(comfortIsOff(toggleComfort(on, "nr"))).toBe(true);
    expect(comfortIsOff(null)).toBe(true);
  });

  it("is not available for WFM", () => {
    expect(comfortSupported("wfm")).toBe(false);
    for (const m of ["nfm", "am", "usb", "lsb", "cw"] as const) {
      expect(comfortSupported(m)).toBe(true);
    }
  });

  it("clamps and snaps the notch", () => {
    expect(clampNotch({ freqHz: 1234, widthHz: 3 })).toEqual({ freqHz: 1230, widthHz: 10 });
    expect(clampNotch({ freqHz: 99_999, widthHz: 5_000 })).toEqual({
      freqHz: 12_000,
      widthHz: 1_000,
    });
  });

  it("maps the audio range of the passband per mode", () => {
    expect(audioRange({ mode: "usb", bandwidthHz: 2_400 })).toEqual({ loHz: 300, hiHz: 2_700 });
    expect(audioRange({ mode: "am", bandwidthHz: 6_000 })).toEqual({ loHz: 0, hiHz: 3_000 });
    expect(audioRange({ mode: "cw", bandwidthHz: 500, cwPitchHz: 700 })).toEqual({
      loHz: 450,
      hiHz: 950,
    });
  });

  it("round-trips the wire form", () => {
    const c = { nb: 0.4, notch: { freqHz: 1200, widthHz: 60 }, autoNotch: 0.5, nr: 0.7 };
    const body = comfortBody(c);
    expect(body).toEqual({
      nb: 0.4,
      notch: { freq_hz: 1200, width_hz: 60 },
      auto_notch: 0.5,
      nr: 0.7,
    });
    expect(parseComfort(body)).toEqual(c);
    expect(comfortBody({})).toEqual({});
    expect(parseComfort({})).toBeNull();
    expect(parseComfort({ notch: { freq_hz: "x" } })).toBeNull();
    expect(parseComfort(null)).toBeNull();
  });

  it("travels in VFO documents and patches", () => {
    const raw = {
      id: 1,
      freq_hz: 7_100_000,
      mode: "lsb",
      bandwidth_hz: 2400,
      squelch_dbfs: null,
      comfort: { nr: 0.6 },
    };
    expect(parseVfo(raw).comfort).toEqual({ nr: 0.6 });
    expect(parseVfo({ ...raw, comfort: undefined })).not.toHaveProperty("comfort");
    expect(patchBody({ comfort: { autoNotch: 0.5 } })).toEqual({
      v: 1,
      comfort: { auto_notch: 0.5 },
    });
    expect(patchBody({ comfort: {} })).toEqual({ v: 1, comfort: {} });
    expect(createBody({ freqHz: 1, mode: "am" })).not.toHaveProperty("comfort");
  });
});
