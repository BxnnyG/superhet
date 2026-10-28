import { describe, expect, it } from "vitest";
import { instance } from "../plugins/testing";
import {
  DIGITAL_MODES,
  digitalModeOfPlugin,
  digitalOnVfo,
  expectText,
  suggestDigital,
} from "./modes";

describe("digital modes", () => {
  it("maps the bundled plugins to modes with a decoder-friendly VFO preset", () => {
    expect(DIGITAL_MODES.map((m) => m.id)).toEqual(["pocsag", "aprs", "ism"]);
    for (const m of DIGITAL_MODES) {
      expect(m.vfo.mode).toBe("nfm");
      expect(m.vfo.squelchDbfs).toBeNull();
      expect(digitalModeOfPlugin(m.pluginId)).toBe(m);
    }
    // rtl_433 wants the whole signal.
    expect(DIGITAL_MODES.find((m) => m.id === "ism")?.vfo.bandwidthHz).toBe(250_000);
    expect(digitalModeOfPlugin("org.example.other")).toBeNull();
  });

  it("suggests the mode that lives on a frequency", () => {
    expect(suggestDigital(144_800_000)?.mode.id).toBe("aprs");
    expect(suggestDigital(144_390_000)?.mode.id).toBe("aprs");
    expect(suggestDigital(433_920_000)?.mode.id).toBe("ism");
    expect(suggestDigital(868_300_000)?.mode.id).toBe("ism");
    expect(suggestDigital(439_987_500)?.mode.id).toBe("pocsag");
    expect(suggestDigital(100_000_000)).toBeNull();
    expect(suggestDigital(145_500_000)).toBeNull();
  });

  it("finds the digital instances of one VFO", () => {
    const list = [
      instance({ id: "a", vfo: 1 }),
      instance({ id: "b", vfo: 2 }),
      instance({ id: "c", vfo: 1, plugin_id: "org.example.other" }),
      instance({ id: "d", vfo: 1, device: "dev1" }),
    ];
    const here = digitalOnVfo(list, "dev0", 1);
    expect(here.map((h) => h.instance.id)).toEqual(["a"]);
    expect(here[0]?.mode.id).toBe("pocsag");
  });

  it("explains what to expect while nothing was decoded", () => {
    const aprs = DIGITAL_MODES.find((m) => m.id === "aprs");
    if (!aprs) throw new Error("aprs");
    const text = expectText(aprs, 144_800_000);
    expect(text).toContain("No packets yet on 144.800 MHz");
    expect(text).toContain("bursty");
  });
});
