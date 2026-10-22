import { expect, it } from "vitest";
import { SQUELCH_MIN_DBFS, autoSquelchDbfs } from "./model";

it("sets the squelch 4 dB above the 20th percentile of the recent levels", () => {
  // 10 samples: sorted index floor(9 * 0.2) = 1 -> -88; +4 -> -84.
  const levels = [-90, -88, -87, -86, -85, -85, -84, -80, -60, -40];
  expect(autoSquelchDbfs(levels)).toBe(-84);
});

it("ignores a transmission in the window (the floor stays the noise)", () => {
  const noise = Array.from({ length: 40 }, (_, i) => -92 + (i % 3));
  const burst = Array.from({ length: 10 }, () => -30);
  expect(autoSquelchDbfs([...noise, ...burst])).toBe(-88);
});

it("is independent of the sample order", () => {
  const a = [-70, -72, -71, -69, -73, -68];
  expect(autoSquelchDbfs([...a].reverse())).toBe(autoSquelchDbfs(a));
});

it("clamps to the slider range", () => {
  expect(autoSquelchDbfs(Array(10).fill(-200))).toBe(SQUELCH_MIN_DBFS);
  expect(autoSquelchDbfs(Array(10).fill(-1))).toBe(0);
});

it("needs enough finite samples", () => {
  expect(autoSquelchDbfs([])).toBeNull();
  expect(autoSquelchDbfs([-80, -80, -80, -80])).toBeNull();
  expect(autoSquelchDbfs([-80, -80, -80, NaN, Infinity, -80])).toBeNull();
});
