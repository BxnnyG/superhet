import { describe, expect, it } from "vitest";
import { fromForm, toForm, type FormState } from "./editorForm";
import { newInput } from "./model";

const base = (over: Partial<FormState> = {}): FormState => ({
  ...toForm(newInput({ name: "Test", freqHz: 145_500_000 })),
  ...over,
});

const input = (over: Partial<FormState>) => {
  const r = fromForm(base(over));
  if ("errors" in r) throw new Error(JSON.stringify(r.errors));
  return r.input;
};
const errors = (over: Partial<FormState>) => {
  const r = fromForm(base(over));
  return "errors" in r ? r.errors : {};
};

describe("editor form", () => {
  it("round-trips a full bookmark", () => {
    const full = newInput({
      name: "Dispatch",
      freqHz: 392_012_500,
      mode: "nfm",
      bandwidthHz: 25_000,
      tags: ["a", "b"],
      color: "#fbbf24",
      notes: "n",
      squelchDb: -75,
      squelchAuto: true,
      stepHz: 25_000,
      toneMode: "dcs",
      dcsCode: 23,
      dcsPolarity: "R",
      digital: {
        protocol: "dmr",
        timeslot: 2,
        talkgroup: 262,
        colourCode: 1,
        mcc: null,
        mnc: null,
        decoderPluginId: "dsd",
      },
      groupId: "g1",
      favourite: true,
      scan: { include: true, priority: 3, dwellMs: 2000 },
    });
    const r = fromForm(toForm(full));
    expect("input" in r && r.input).toEqual(full);
  });

  it("keeps only the fields of the chosen sub-tone", () => {
    const ctcss = input({ toneMode: "ctcss", ctcss: "88.5", dcsCode: "023" });
    expect(ctcss).toMatchObject({
      toneMode: "ctcss",
      ctcssHz: 88.5,
      dcsCode: null,
      dcsPolarity: null,
    });
    const dcs = input({ toneMode: "dcs", ctcss: "88.5", dcsCode: "047", dcsPolarity: "R" });
    expect(dcs).toMatchObject({ toneMode: "dcs", ctcssHz: null, dcsCode: 47, dcsPolarity: "R" });
    expect(input({ toneMode: "" })).toMatchObject({ toneMode: null, ctcssHz: null });
  });

  it("validates tones, squelch and digital fields", () => {
    expect(errors({ toneMode: "ctcss", ctcss: "" }).ctcss).toBeDefined();
    expect(errors({ toneMode: "dcs", dcsCode: "089" }).dcs).toBeDefined();
    expect(errors({ toneMode: "dcs", dcsCode: "000" }).dcs).toBeDefined();
    expect(errors({ toneMode: "dcs", dcsCode: "1234" }).dcs).toBeDefined();
    expect(errors({ squelchKind: "fixed", squelch: "" }).squelch).toBeDefined();
    expect(errors({ squelchKind: "fixed", squelch: "5" }).squelch).toBeDefined();
    expect(errors({ squelchKind: "auto", squelch: "" }).squelch).toBeUndefined();
    expect(errors({ protocol: "tetra", timeslot: "5" }).timeslot).toBeDefined();
    expect(errors({ protocol: "dmr", timeslot: "3" }).timeslot).toBeDefined();
    expect(errors({ protocol: "dmr", colourCode: "16" }).colourCode).toBeDefined();
    expect(errors({ protocol: "tetra", talkgroup: "x" }).talkgroup).toBeDefined();
    expect(errors({ protocol: "tetra", decoder: "bad id" }).decoder).toBeDefined();
    expect(errors({ scanDwell: "5" }).scanDwell).toBeDefined();
    expect(errors({ name: " ", freq: "abc", bandwidth: "-3" })).toMatchObject({
      name: expect.any(String) as string,
      freq: expect.any(String) as string,
      bandwidth: expect.any(String) as string,
    });
  });

  it("drops the timeslot and colour code that do not belong to the protocol", () => {
    const p25 = input({ protocol: "p25", timeslot: "1", colourCode: "3", talkgroup: "5" });
    expect(p25.digital).toMatchObject({
      protocol: "p25",
      timeslot: null,
      colourCode: null,
      talkgroup: 5,
    });
    expect(input({ protocol: "" }).digital).toBeNull();
  });
});
