import { describe, expect, it } from "vitest";
import { AIS_PLUGIN_ID } from "../ais/format";
import { ACARS_PLUGIN_ID } from "../datalink/model";
import { DIGITAL_MODES, digitalMode } from "../digital/modes";
import { DSD_PLUGIN_ID } from "../dsd/model";
import { instance } from "../plugins/testing";
import type { Vfo } from "../vfo/api";
import { activeChannel, channelOf, draftFor, modeOfProtocol } from "./digital";
import {
  DIGITAL_PROTOCOLS,
  PROTOCOL_LABEL,
  bookmarkTarget,
  findSameChannel,
  newInput,
  receiverPatch,
  type Bookmark,
} from "./model";

const vfo: Vfo = {
  id: 1,
  freqHz: 439_987_500,
  mode: "nfm",
  bandwidthHz: 12_500,
  squelchDbfs: -75,
  deemphasis: null,
  toneSquelchHz: 88.5,
  offsetHz: 0,
  status: "active",
};

const saved = (id: string, over: Partial<Bookmark> = {}): Bookmark => ({
  ...newInput({ name: id, freqHz: 439_987_500, mode: "nfm" }),
  id,
  createdMs: 1,
  updatedMs: 1,
  ...over,
});

/** Picker modes the stored protocol list (a database constraint, ADR-0031) does not know yet. */
const NOT_STORED: readonly string[] = ["ais", "acars", "vdl2"];

describe("digital modes in bookmarks", () => {
  it("offers every mode of the Digital picker as a protocol", () => {
    for (const m of DIGITAL_MODES.filter((m) => !NOT_STORED.includes(m.id))) {
      expect(DIGITAL_PROTOCOLS).toContain(m.id);
      expect(PROTOCOL_LABEL[m.id as keyof typeof PROTOCOL_LABEL]).toBe(m.label);
      expect(modeOfProtocol(m.id)?.pluginId).toBe(m.pluginId);
    }
    // P25 is stored but has no decoder to start.
    expect(modeOfProtocol("p25")).toBeNull();
  });

  it("saves a bookmark made while AIS or ACARS runs as an analogue one", () => {
    expect(channelOf(digitalMode("ais"))).toBeNull();
    expect(channelOf(digitalMode("vdl2"))).toBeNull();
    const ais = instance({ plugin_id: AIS_PLUGIN_ID, vfo: 1 });
    const acars = instance({ id: "i-2", plugin_id: ACARS_PLUGIN_ID, vfo: 1 });
    expect(activeChannel([ais, acars], "dev0", 1)).toBeNull();
    // A storable mode on the same receiver still wins.
    const pocsag = instance({ id: "i-3", vfo: 1 });
    expect(activeChannel([ais, pocsag], "dev0", 1)?.protocol).toBe("pocsag");
  });

  it("saves the digital mode running on the VFO and restores it from the bookmark", () => {
    const pocsag = instance({ vfo: 1 });
    expect(activeChannel([pocsag], "dev0", 1)).toMatchObject({
      protocol: "pocsag",
      decoderPluginId: pocsag.pluginId,
    });
    // Another VFO, another device, no VFO: analogue.
    expect(activeChannel([pocsag], "dev0", 2)).toBeNull();
    expect(activeChannel([pocsag], "dev1", 1)).toBeNull();
    expect(activeChannel([pocsag], "dev0", null)).toBeNull();
    // DMR / dPMR / NXDN share one plugin; the instance's config tells them apart.
    const dpmr = instance({ plugin_id: DSD_PLUGIN_ID, config: { protocol: "dpmr" } });
    expect(activeChannel([dpmr], "dev0", 1)?.protocol).toBe("dpmr");
    // The saved channel is what loading the bookmark acts on.
    const target = bookmarkTarget({
      ...saved("a", { digital: channelOf(modeOfProtocol("nxdn")!) }),
    });
    expect(target.digital).toBe("nxdn");
    expect(bookmarkTarget(saved("b")).digital).toBeUndefined();
  });

  it("prefills the dialog from the active VFO, digital mode included", () => {
    const draft = draftFor({
      freqHz: vfo.freqHz,
      planName: "",
      vfo,
      squelchIsAuto: false,
      step: 12_500,
      digital: channelOf(modeOfProtocol("pocsag")!),
    });
    expect(draft).toMatchObject({
      name: "POCSAG 439.987 MHz",
      freqHz: 439_987_500,
      mode: "nfm",
      bandwidthHz: 12_500,
      squelchDb: -75,
      squelchAuto: false,
      stepHz: 12_500,
      toneMode: "ctcss",
      ctcssHz: 88.5,
      digital: { protocol: "pocsag", decoderPluginId: modeOfProtocol("pocsag")!.pluginId },
    });
    // A band plan name wins; analogue stays analogue; no VFO still gives a usable draft.
    expect(
      draftFor({
        freqHz: 1e8,
        planName: "FM",
        vfo,
        squelchIsAuto: true,
        step: "auto",
        digital: null,
      }),
    ).toMatchObject({ name: "FM", digital: null, squelchAuto: true, stepHz: null });
    expect(
      draftFor({
        freqHz: 1e8,
        planName: "FM",
        vfo,
        squelchIsAuto: false,
        step: "auto",
        digital: channelOf(modeOfProtocol("aprs")!),
      }).name,
    ).toBe("FM (APRS)");
    expect(
      draftFor({
        freqHz: 1e8,
        planName: "",
        vfo: null,
        squelchIsAuto: false,
        step: "auto",
        digital: null,
      }),
    ).toMatchObject({ name: "100.000 MHz", mode: null, bandwidthHz: null });
  });
});

describe("same channel", () => {
  const pocsag = channelOf(modeOfProtocol("pocsag")!);

  it("matches on frequency, mode and digital mode, not on the name", () => {
    const list = [saved("a"), saved("b", { digital: pocsag }), saved("c", { freqHz: 1e8 })];
    expect(findSameChannel(list, { freqHz: 439_987_500.4, mode: "nfm", digital: null })?.id).toBe(
      "a",
    );
    expect(findSameChannel(list, { freqHz: 439_987_500, mode: "nfm", digital: pocsag })?.id).toBe(
      "b",
    );
    expect(findSameChannel(list, { freqHz: 439_987_500, mode: "am", digital: null })).toBeNull();
    expect(findSameChannel(list, { freqHz: 439_987_600, mode: "nfm", digital: null })).toBeNull();
    expect(
      findSameChannel(list, { freqHz: 439_987_500, mode: "nfm", digital: null }, "a"),
    ).toBeNull();
  });

  it("updates only the receiver state of the existing bookmark", () => {
    const patch = receiverPatch(
      newInput({
        name: "new name",
        freqHz: 1,
        mode: "am",
        tags: ["x"],
        digital: pocsag,
        squelchDb: -60,
      }),
    );
    expect(patch).toMatchObject({ freqHz: 1, mode: "am", squelchDb: -60, digital: pocsag });
    for (const kept of ["name", "tags", "notes", "color", "groupId", "favourite", "scan"]) {
      expect(patch).not.toHaveProperty(kept);
    }
  });
});
