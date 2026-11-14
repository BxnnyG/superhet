import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { AdsbStore } from "../adsb/store";
import { adsbReply, fakeAdsbApi } from "../adsb/testing";
import type { EventApi } from "../decoders/api";
import { parseEvent, type DecoderEvent } from "../decoders/events";
import { describeEvent } from "../decoders/render";
import { DecoderEventStore } from "../decoders/store";
import { DigitalDock } from "../digital/DigitalDock";
import { DIGITAL_MODES, digitalModeOfPlugin, suggestDigital } from "../digital/modes";
import { parseManifest } from "../plugins/api";
import { PluginStore } from "../plugins/store";
import { fakePluginApi, instance } from "../plugins/testing";
import { vfo } from "../shell/fixtures";
import { toasts } from "../ui";
import {
  ACARS_MESSAGE,
  ACARS_PLUGIN_ID,
  VDL2_MESSAGE,
  VDL2_PLUGIN_ID,
  aircraftRef,
  findAircraft,
  labelText,
  normalizeReg,
} from "./model";

const manifest = (id: string, name: string, bandwidth: number, rate: number) =>
  parseManifest({
    plugin: {
      id,
      name,
      version: "0.1.0",
      protocol: 1,
      license: "AGPL-3.0-only",
      type: "decoder",
      description: name,
    },
    runtime: { kind: "process", transport: "simple", command: ["superhet-plugin-adapter", "x"] },
    input: { kind: "iq", sample_rate: rate, format: "cu8", bandwidth_hz: bandwidth },
    device: { exclusive: false },
    install: { acknowledge: { text: "Aircraft send identifiers." } },
  });
const acarsPlugin = manifest(ACARS_PLUGIN_ID, "ACARS messages", 200_000, 240_000);
const vdl2Plugin = manifest(VDL2_PLUGIN_ID, "VDL Mode 2", 300_000, 420_000);

const ACARS = {
  freq_mhz: 131.725,
  label: "H1",
  tail: "D-AIXA",
  flight: "LH1234",
  direction: "downlink",
  text: "#CFBFLT01/PSN\r\n4811.2N01131.5E",
  summary: "D-AIXA LH1234 [H1] #CFBFLT01/PSN 4811.2N01131.5E",
};
const VDL2 = {
  kind: "acars",
  freq_mhz: 136.975,
  icao: "3c6444",
  src_addr: "3c6444",
  src_type: "Aircraft",
  dst_addr: "22a1b3",
  dst_type: "Ground station",
  label: "Q0",
  summary: "[Q0]",
};

function ev(id: number, kind: string, source: string, data: Record<string, unknown>) {
  const e = parseEvent({
    id,
    ts_ms: 1_700_000_000_000 + id * 1000,
    device: "dev0",
    vfo: 1,
    freq_hz: 131_725_000,
    mode: "nfm",
    kind,
    source,
    value: "x",
    meta: { instance: "i-1", data },
  });
  if (!e) throw new Error("fixture");
  return e;
}

describe("aircraft references", () => {
  it("reads the address and the registration of a message", () => {
    expect(aircraftRef({ icao: "3C6444", tail: "D-AIXA" })).toEqual({
      icao: "3c6444",
      reg: "D-AIXA",
    });
    expect(aircraftRef({ tail: "D-AIXA" })).toEqual({ icao: null, reg: "D-AIXA" });
    expect(aircraftRef({ icao: "xyz" })).toBeNull();
    expect(aircraftRef({})).toBeNull();
  });

  it("compares registrations without dashes, dots and case", () => {
    expect(normalizeReg(".d-aixa")).toBe("DAIXA");
    expect(normalizeReg("N123AB")).toBe("N123AB");
  });

  it("finds an aircraft by address first, then by registration from the database", () => {
    const { aircraft } = adsbReply().status ?? { aircraft: [] };
    const info = (icao: string) =>
      icao === "406a3d"
        ? {
            registration: "G-EZTH",
            typeCode: null,
            typeDesc: null,
            typeClass: null,
            wake: null,
            military: false,
          }
        : null;
    expect(findAircraft(aircraft, info, { icao: "3c6444", reg: null })?.icao).toBe("3c6444");
    expect(findAircraft(aircraft, info, { icao: null, reg: "g-ezth" })?.icao).toBe("406a3d");
    expect(findAircraft(aircraft, info, { icao: "ffffff", reg: "G-EZTH" })?.icao).toBe("406a3d");
    expect(findAircraft(aircraft, info, { icao: null, reg: "D-ZZZZ" })).toBeNull();
    expect(findAircraft(aircraft, info, { icao: null, reg: "--" })).toBeNull();
  });

  it("knows what a few labels mean", () => {
    expect(labelText("Q0")).toBe("Link test");
    expect(labelText("ZZ")).toBeNull();
    expect(labelText(null)).toBeNull();
  });
});

describe("the log rows", () => {
  it("describes an ACARS message with registration, flight, label and text", () => {
    const v = describeEvent(ev(1, ACARS_MESSAGE, ACARS_PLUGIN_ID, ACARS));
    expect(v.label).toBe("ACARS");
    expect(v.headline).toBe("D-AIXA");
    expect(v.tags).toEqual(["LH1234", "H1"]);
    expect(v.detail).toBe("#CFBFLT01/PSN 4811.2N01131.5E");
    expect(v.fields.find((f) => f.label === "Label")?.value).toBe(
      "H1 (Message to or from a terminal)",
    );
    expect(v.fields.find((f) => f.label === "Direction")?.value).toBe("aircraft to ground");
    expect(v.fields.find((f) => f.label === "Frequency")?.value).toBe("131.725 MHz");
    expect(v.aircraft).toEqual({ icao: null, reg: "D-AIXA" });
  });

  it("describes a VDL2 message by its ICAO address when it has no registration", () => {
    const v = describeEvent(ev(2, VDL2_MESSAGE, VDL2_PLUGIN_ID, VDL2));
    expect(v.label).toBe("VDL2");
    expect(v.headline).toBe("3C6444");
    expect(v.tags).toEqual(["Q0"]);
    expect(v.fields.find((f) => f.label === "From")?.value).toBe("3C6444 (Aircraft)");
    expect(v.fields.find((f) => f.label === "To")?.value).toBe("22A1B3 (Ground station)");
    expect(v.aircraft).toEqual({ icao: "3c6444", reg: null });
  });

  it("shows decoded ADS-C / CPDLC content in the details", () => {
    const v = describeEvent(
      ev(3, VDL2_MESSAGE, VDL2_PLUGIN_ID, {
        kind: "x25",
        icao: "3c6444",
        summary: "3C6444 X.25 data",
        decoded: { clnp: { cpdlc: { msg: "CLEARED" } } },
      }),
    );
    expect(v.detail).toBe("3C6444 X.25 data");
    expect(v.fields.find((f) => f.label === "Decoded content")?.value).toContain("CLEARED");
  });
});

describe("the digital modes", () => {
  it("offers AIS, ACARS and VDL2 on receivers with the bandwidth their decoders need", () => {
    const by = (id: string) => DIGITAL_MODES.find((m) => m.id === id);
    expect(by("ais")?.vfo.bandwidthHz).toBe(150_000);
    expect(by("acars")?.vfo.bandwidthHz).toBe(200_000);
    expect(by("vdl2")?.vfo.bandwidthHz).toBe(300_000);
    expect(digitalModeOfPlugin(ACARS_PLUGIN_ID)?.id).toBe("acars");
    expect(digitalModeOfPlugin(VDL2_PLUGIN_ID)?.id).toBe("vdl2");
  });

  it("suggests them on their frequencies", () => {
    expect(suggestDigital(162_000_000)?.mode.id).toBe("ais");
    expect(suggestDigital(131_725_000)?.mode.id).toBe("acars");
    expect(suggestDigital(136_975_000)?.mode.id).toBe("vdl2");
    expect(suggestDigital(145_500_000)).toBeNull();
  });
});

describe("the output panel", () => {
  beforeEach(() => {
    localStorage.clear();
    toasts.clear();
  });
  afterEach(() => vi.useRealTimers());

  const flush = (ms = 0) => act(() => new Promise<void>((r) => setTimeout(r, ms)));

  async function setup(pluginId: string, events: DecoderEvent[]) {
    const fake = fakePluginApi(
      [acarsPlugin, vdl2Plugin],
      [instance({ plugin_id: pluginId, freq_hz: 131_725_000 })],
    );
    const plugins = new PluginStore(fake.api);
    const eventApi = {
      list: vi.fn<EventApi["list"]>(() => Promise.resolve({ events, next: null })),
      clear: vi.fn<EventApi["clear"]>(() => Promise.resolve(0)),
    } satisfies EventApi;
    const store = new DecoderEventStore({
      device: "dev0",
      api: eventApi,
      createSocket: () => ({ close() {} }) as unknown as WebSocket,
    });
    const adsbApi = fakeAdsbApi();
    const adsb = new AdsbStore(adsbApi.api);
    adsb.start();
    adsb.follow("i-adsb");
    render(
      <DigitalDock
        plugins={plugins}
        device="dev0"
        vfos={[vfo(1, { freqHz: 131_725_000 })]}
        selectedId={1}
        createEvents={() => store}
        adsb={adsb}
      />,
    );
    plugins.start();
    await flush();
    // The registrations come from the aircraft database after a short debounce.
    await flush(250);
    return { plugins, adsb };
  }

  it("lists ACARS messages and links the one whose registration is on the ADS-B map", async () => {
    const { plugins, adsb } = await setup(ACARS_PLUGIN_ID, [
      ev(2, ACARS_MESSAGE, ACARS_PLUGIN_ID, ACARS),
      ev(1, ACARS_MESSAGE, ACARS_PLUGIN_ID, { ...ACARS, tail: "D-ZZZZ", flight: "XX1" }),
    ]);
    const dock = screen.getByTestId("digital-dock");
    expect(within(dock).getByText("Aircraft")).toBeInTheDocument();
    const rows = within(dock).getAllByRole("row");
    expect(rows).toHaveLength(3);
    expect(rows[1]).toHaveTextContent("D-AIXA");
    expect(rows[1]).toHaveTextContent("LH1234");
    // Only the aircraft the ADS-B decoder sees (D-AIXA is 3c6444 in the database) gets the button.
    expect(
      within(rows[1] as HTMLElement).getByRole("button", { name: "On the ADS-B map" }),
    ).toBeEnabled();
    expect(
      within(rows[2] as HTMLElement).queryByRole("button", { name: "On the ADS-B map" }),
    ).toBeNull();
    fireEvent.click(
      within(rows[1] as HTMLElement).getByRole("button", { name: "On the ADS-B map" }),
    );
    expect(adsb.getSnapshot()).toMatchObject({ viewOpen: true, focus: "3c6444" });
    plugins.stop();
    adsb.stop();
  });

  it("links a VDL2 message by the ICAO address without any database", async () => {
    const { plugins, adsb } = await setup(VDL2_PLUGIN_ID, [
      ev(1, VDL2_MESSAGE, VDL2_PLUGIN_ID, { ...VDL2, icao: "406a3d" }),
    ]);
    const rows = within(screen.getByTestId("digital-dock")).getAllByRole("row");
    const button = within(rows[1] as HTMLElement).getByRole("button", { name: "On the ADS-B map" });
    expect(button).toHaveAttribute("title", "Show 406A3D on the ADS-B map");
    fireEvent.click(button);
    expect(adsb.getSnapshot().focus).toBe("406a3d");
    plugins.stop();
    adsb.stop();
  });
});
