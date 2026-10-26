import { expect, it } from "vitest";
import { createFor, patchFor } from "./annotationTuning";

it("switches mode and bandwidth when the mode differs", () => {
  expect(
    patchFor({ mode: "wfm" }, { freqHz: 446_031_250, mode: "nfm", bandwidthHz: 12_500 }),
  ).toEqual({ freqHz: 446_031_250, mode: "nfm", bandwidthHz: 12_500 });
  expect(patchFor({ mode: "wfm" }, { freqHz: 118_300_000, mode: "am" })).toEqual({
    freqHz: 118_300_000,
    mode: "am",
    bandwidthHz: null,
  });
});

it("keeps a hand-set bandwidth when only the frequency is given", () => {
  expect(patchFor({ mode: "nfm" }, { freqHz: 145_500_000 })).toEqual({ freqHz: 145_500_000 });
  expect(patchFor({ mode: "nfm" }, { freqHz: 145_500_000, mode: "nfm" })).toEqual({
    freqHz: 145_500_000,
  });
  expect(
    patchFor({ mode: "nfm" }, { freqHz: 145_500_000, mode: "nfm", bandwidthHz: 25_000 }),
  ).toEqual({ freqHz: 145_500_000, bandwidthHz: 25_000 });
});

it("creates a VFO with the target's mode or the default for the frequency", () => {
  expect(createFor({ freqHz: 100_100_000 })).toEqual({ freqHz: 100_100_000, mode: "wfm" });
  expect(createFor({ freqHz: 433_500_000, mode: "nfm", bandwidthHz: 12_500 })).toEqual({
    freqHz: 433_500_000,
    mode: "nfm",
    bandwidthHz: 12_500,
  });
});

it("applies squelch and the CTCSS tone squelch, the tone only to NFM and AM", () => {
  const target = {
    freqHz: 145_500_000,
    mode: "nfm" as const,
    squelchDbfs: -80,
    toneSquelchHz: 88.5,
  };
  expect(patchFor({ mode: "nfm" }, target)).toEqual({
    freqHz: 145_500_000,
    squelchDbfs: -80,
    toneSquelchHz: 88.5,
  });
  // Switching from WFM to NFM: the tone rides along with the mode change.
  expect(patchFor({ mode: "wfm" }, target)).toMatchObject({ mode: "nfm", toneSquelchHz: 88.5 });
  // A WFM target has no tone squelch.
  expect(
    patchFor({ mode: "nfm" }, { freqHz: 100_100_000, mode: "wfm", toneSquelchHz: null }),
  ).toEqual({ freqHz: 100_100_000, mode: "wfm", bandwidthHz: null });
  // "No tone" switches a left-over tone squelch off.
  expect(patchFor({ mode: "nfm" }, { freqHz: 1, toneSquelchHz: null })).toEqual({
    freqHz: 1,
    toneSquelchHz: null,
  });
  expect(createFor(target)).toEqual({
    freqHz: 145_500_000,
    mode: "nfm",
    squelchDbfs: -80,
    toneSquelchHz: 88.5,
  });
  expect(createFor({ freqHz: 100_100_000, toneSquelchHz: 88.5 })).toEqual({
    freqHz: 100_100_000,
    mode: "wfm",
  });
});
