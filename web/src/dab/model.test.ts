import { describe, expect, it } from "vitest";
import { builtinPlans } from "../channels/catalog";
import {
  DAB_BLOCKS,
  blockFreq,
  blockOf,
  latestEnsembles,
  parseReply,
  parseStatus,
  signalGrade,
  slideKey,
} from "./model";
import { dabReplyRaw, dabStatusRaw } from "./testing";

describe("DAB blocks", () => {
  it("are the blocks of the built-in channel plan", () => {
    const plan = builtinPlans().find((p) => p.id === "dab-band3");
    expect(plan).toBeDefined();
    expect(DAB_BLOCKS.map(([name, freqHz]) => ({ number: name, freqHz }))).toEqual(
      plan?.channels.map((c) => ({ number: c.number, freqHz: c.freqHz })),
    );
  });

  it("map names and frequencies both ways", () => {
    expect(blockFreq("5C")).toBe(178_352_000);
    expect(blockFreq("nope")).toBeNull();
    expect(blockOf(178_350_000)).toBe("5C");
    expect(blockOf(100_000_000)).toBeNull();
    expect(blockOf(null)).toBeNull();
  });
});

describe("status parsing", () => {
  it("reads the plugin's state document", () => {
    const status = parseStatus(dabStatusRaw());
    expect(status).toMatchObject({
      mode: "tuned",
      channel: "5C",
      synced: true,
      selected: "D210",
      playing: true,
      ensemble: { label: "DR Deutschland", eid: "D210" },
    });
    expect(status?.services).toHaveLength(2);
    expect(status?.services[0]).toMatchObject({
      sid: "D210",
      label: "Deutschlandfunk",
      codec: "DAB+",
      bitrate: 72,
      audio: true,
      dls: "Nachrichten",
    });
    expect(status?.services[1]?.dls).toBeNull();
  });

  it("reads scan progress and results", () => {
    const status = parseStatus(
      dabStatusRaw({
        mode: "scanning",
        scan: {
          index: 3,
          total: 38,
          channel: "6A",
          found: [{ channel: "5C", freq_hz: 178352000, label: "DR", eid: "D210", services: 8 }],
        },
      }),
    );
    expect(status?.scan).toMatchObject({ index: 3, total: 38, channel: "6A" });
    expect(status?.scan?.found[0]).toMatchObject({ channel: "5C", services: 8, snr: null });
  });

  it("is tolerant: wrong shapes give null or defaults, never a throw", () => {
    expect(parseStatus(null)).toBeNull();
    expect(parseStatus({ v: 2 })).toBeNull();
    expect(parseStatus({ v: 1, services: [{ nonsense: true }, 5] })?.services).toEqual([]);
    expect(parseStatus({ v: 1 })?.mode).toBe("idle");
  });

  it("parses the status reply with media and the audio stream id", () => {
    const reply = parseReply(dabReplyRaw());
    expect(reply).toMatchObject({
      pluginId: "org.superhet.decoder.dab",
      state: "running",
      audioVfo: 1073741900,
      freqHz: 178352000,
    });
    expect(reply.media).toEqual([{ key: "slide.d210", mime: "image/png", size: 8, revision: 2 }]);
    expect(() => parseReply({})).toThrow();
    expect(parseReply({ plugin_id: "x", status: null }).status).toBeNull();
  });
});

describe("helpers", () => {
  it("grade the signal from SNR and sync", () => {
    const base = parseStatus(dabStatusRaw());
    expect(signalGrade(base)).toBe("good");
    expect(signalGrade(parseStatus(dabStatusRaw({ snr: 8 })))).toBe("fair");
    expect(signalGrade(parseStatus(dabStatusRaw({ snr: 2 })))).toBe("poor");
    expect(signalGrade(parseStatus(dabStatusRaw({ synced: false })))).toBe("none");
    expect(signalGrade(null)).toBe("none");
  });

  it("name the slide of a service", () => {
    expect(slideKey("D210")).toBe("slide.d210");
  });

  it("keep the newest ensemble per block from the history, sorted by frequency", () => {
    const event = (channel: string, label: string, services: number) => ({
      kind: "dab.ensemble",
      meta: {
        data: { channel, freq_hz: blockFreq(channel), label, eid: "D2", services: Array(services) },
      },
    });
    const found = latestEnsembles([
      event("6A", "New 6A", 5),
      { kind: "dab.dls", meta: {} },
      event("5C", "Five C", 8),
      event("6A", "Old 6A", 2),
    ]);
    expect(found.map((f) => [f.channel, f.label, f.services])).toEqual([
      ["5C", "Five C", 8],
      ["6A", "New 6A", 5],
    ]);
  });
});
