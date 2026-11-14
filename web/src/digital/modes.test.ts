import { describe, expect, it } from "vitest";
import { FT8_PLUGIN_ID as FT8_ID } from "../ft8/model";
import { instance } from "../plugins/testing";
import {
  DIGITAL_MODES,
  digitalModeOfInstance,
  digitalModeOfPlugin,
  digitalOnVfo,
  expectText,
  suggestDigital,
} from "./modes";

describe("digital modes", () => {
  it("maps the bundled plugins to modes with a decoder-friendly VFO preset", () => {
    expect(DIGITAL_MODES.map((m) => m.id)).toEqual([
      "pocsag",
      "aprs",
      "ism",
      "m17",
      "dmr",
      "dpmr",
      "nxdn",
      "tetra",
      "ft8",
      "ft4",
      "wspr",
    ]);
    for (const m of DIGITAL_MODES) {
      // The weak-signal modes decode the audio of an upper-sideband receiver.
      expect(m.vfo.mode).toBe(["ft8", "ft4", "wspr"].includes(m.id) ? "usb" : "nfm");
      expect(m.vfo.squelchDbfs).toBeNull();
      // DMR, dPMR and NXDN share one plugin; the first of them stands for it.
      expect(digitalModeOfPlugin(m.pluginId)?.pluginId).toBe(m.pluginId);
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
    // M17 has no calling frequency to suggest.
    expect(DIGITAL_MODES.find((m) => m.id === "m17")?.hints).toEqual([]);
    expect(suggestDigital(145_500_000)).toBeNull();
  });

  it("suggests FT8, FT4 and WSPR on their dial frequencies", () => {
    expect(suggestDigital(14_074_000)?.mode.id).toBe("ft8");
    expect(suggestDigital(14_080_000)?.mode.id).toBe("ft4");
    expect(suggestDigital(14_095_600)?.mode.id).toBe("wspr");
    expect(suggestDigital(144_174_000)?.mode.id).toBe("ft8");
    expect(suggestDigital(14_150_000)).toBeNull();
  });

  it("tells FT8, FT4 and WSPR instances of one plugin apart by their config", () => {
    const of = (mode: string) =>
      digitalModeOfInstance(instance({ plugin_id: FT8_ID, config: { mode } }))?.id;
    expect(of("ft8")).toBe("ft8");
    expect(of("ft4")).toBe("ft4");
    expect(of("wspr")).toBe("wspr");
    expect(digitalModeOfInstance(instance({ plugin_id: FT8_ID, config: {} }))?.id).toBe("ft8");
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
