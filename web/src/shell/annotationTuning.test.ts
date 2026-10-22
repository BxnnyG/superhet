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
