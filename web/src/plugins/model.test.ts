import { beforeEach, describe, expect, it } from "vitest";
import { vfo } from "../shell/fixtures";
import {
  CONFIG_FIELDS,
  buildConfig,
  configToDraft,
  defaultDraft,
  describeBinding,
  describeClaim,
  describeInput,
  instanceProblem,
  isBundled,
  noticeAccepted,
  parseConfigJson,
  rememberNotice,
  suggestVfo,
} from "./model";
import { ADSB_ID, POCSAG_ID, RTL433_ID, adsb, instance, pocsag, rtl433 } from "./testing";

const fields = (id: string) => CONFIG_FIELDS[id] ?? [];

describe("config forms", () => {
  it("sends nothing for the defaults", () => {
    const f = fields(POCSAG_ID);
    expect(buildConfig(f, defaultDraft(f))).toEqual({ ok: true, config: {} });
  });

  it("sends only what differs from the defaults", () => {
    const f = fields(POCSAG_ID);
    const draft = {
      ...defaultDraft(f),
      baud: [1200, 512],
      flex: false,
      invert: "inverted",
      charset: "DE",
    };
    expect(buildConfig(f, draft)).toEqual({
      ok: true,
      config: { baud: [512, 1200], flex: false, invert: true, charset: "DE" },
    });
  });

  it("maps the polarity choice to the config and back", () => {
    const f = fields(POCSAG_ID);
    expect(buildConfig(f, { ...defaultDraft(f), invert: "normal" })).toEqual({
      ok: true,
      config: { invert: false },
    });
    expect(configToDraft(f, { invert: false }).invert).toBe("normal");
    expect(configToDraft(f, { invert: true }).invert).toBe("inverted");
    expect(configToDraft(f, {}).invert).toBe("");
  });

  it("turns a stored config back into form state", () => {
    const f = fields(POCSAG_ID);
    const config = { baud: [512, 1200], flex: false, charset: "DE" };
    const draft = configToDraft(f, config);
    expect(draft).toMatchObject({ baud: [512, 1200], flex: false, charset: "DE", invert: "" });
    expect(buildConfig(f, draft)).toEqual({ ok: true, config });
    const r = fields(RTL433_ID);
    expect(configToDraft(r, { protocols: [19, 20] })).toEqual({ protocols: "19, 20" });
    expect(configToDraft(f, null)).toEqual(defaultDraft(f));
    expect(configToDraft(f, [1])).toEqual(defaultDraft(f));
  });

  it("treats all baud rates like none", () => {
    const f = fields(POCSAG_ID);
    expect(buildConfig(f, { ...defaultDraft(f), baud: [512, 1200, 2400] })).toEqual({
      ok: true,
      config: {},
    });
  });

  it("parses the rtl_433 protocol list and rejects junk", () => {
    const f = fields(RTL433_ID);
    expect(buildConfig(f, { protocols: "19, 20;21  22" })).toEqual({
      ok: true,
      config: { protocols: [19, 20, 21, 22] },
    });
    expect(buildConfig(f, { protocols: "19, x" })).toMatchObject({ ok: false });
    expect(buildConfig(f, { protocols: "-1" })).toMatchObject({ ok: false });
    expect(buildConfig(f, { protocols: " " })).toEqual({ ok: true, config: {} });
  });

  it("validates the free-form JSON of other plugins", () => {
    expect(parseConfigJson("")).toEqual({ ok: true, config: {} });
    expect(parseConfigJson('{"mode":"crash"}')).toEqual({ ok: true, config: { mode: "crash" } });
    expect(parseConfigJson("[1]")).toMatchObject({ ok: false });
    expect(parseConfigJson("{nope")).toMatchObject({ ok: false });
  });
});

describe("VFO suggestions", () => {
  it("asks for the width rtl_433 needs", () => {
    const s = suggestVfo(rtl433, vfo(1, { mode: "nfm", bandwidthHz: 12_500 }));
    expect(s?.patch).toEqual({ bandwidthHz: 250_000 });
    expect(s?.lines[0]).toContain("250 kHz");
    expect(suggestVfo(rtl433, vfo(1, { bandwidthHz: 250_000 }))).toBeNull();
    // A little narrower is accepted.
    expect(suggestVfo(rtl433, vfo(1, { bandwidthHz: 240_000 }))).toBeNull();
  });

  it("asks for NFM, its width and an open squelch for a discriminator decoder", () => {
    const s = suggestVfo(pocsag, vfo(1, { mode: "wfm", bandwidthHz: 180_000, squelchDbfs: -60 }));
    expect(s?.patch).toEqual({ mode: "nfm", bandwidthHz: 12_500, squelchDbfs: null });
    expect(s?.lines).toHaveLength(3);
  });

  it("is happy with an NFM channel and an open squelch", () => {
    expect(suggestVfo(pocsag, vfo(1))).toBeNull();
    expect(suggestVfo(pocsag, vfo(1, { squelchDbfs: -50 }))?.patch).toEqual({ squelchDbfs: null });
  });

  it("has nothing to say for exclusive plugins", () => {
    expect(suggestVfo(adsb, vfo(1))).toBeNull();
  });
});

describe("notice memory", () => {
  beforeEach(() => localStorage.clear());

  it("asks until the exact text was confirmed", () => {
    expect(noticeAccepted(pocsag)).toBe(false);
    rememberNotice(pocsag);
    expect(noticeAccepted(pocsag)).toBe(true);
    // A changed notice has to be read again.
    const changed = {
      ...pocsag,
      acknowledge: { text: "new text", url: null },
    };
    expect(noticeAccepted(changed)).toBe(false);
  });

  it("needs nothing for a plugin without a notice", () => {
    expect(noticeAccepted(adsb)).toBe(true);
  });

  it("survives broken storage", () => {
    localStorage.setItem("superhet.plugin-notices", "{broken");
    expect(noticeAccepted(pocsag)).toBe(false);
  });
});

describe("descriptions", () => {
  it("describes input, claim, binding and problems", () => {
    expect(describeInput(pocsag)).toBe("discriminator 22.05 kHz s16le");
    expect(describeInput(rtl433)).toBe("IQ 250 kHz cu8");
    expect(describeClaim(adsb)).toBe("1090.000 MHz");
    expect(describeClaim(pocsag)).toBeNull();
    expect(describeBinding(instance())).toBe("VFO 1 · dev0 · 439.9875 MHz");
    expect(describeBinding(instance({ vfo: null, exclusive: true, freq_hz: 1090e6 }))).toBe(
      "whole device · dev0 · 1090.000 MHz",
    );
    expect(instanceProblem(instance())).toBeNull();
    expect(instanceProblem(instance({ state: "error", last_error: "exit status 3" }))).toBe(
      "exit status 3",
    );
    expect(instanceProblem(instance({ input_error: "unknown vfo" }))).toBe("Input: unknown vfo");
  });

  it("marks the first-party plugins as bundled", () => {
    expect(isBundled(POCSAG_ID)).toBe(true);
    expect(isBundled(ADSB_ID)).toBe(false);
  });
});
