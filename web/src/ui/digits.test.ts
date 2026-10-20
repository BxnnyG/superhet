import { describe, expect, it } from "vitest";
import {
  DEFAULT_MAX_HZ,
  digitsOf,
  leadingZeros,
  placeLabel,
  placeValue,
  setDigit,
  stepDigit,
  wheelPixels,
} from "./digits";

describe("digit helpers", () => {
  it("maps indexes to place values and labels", () => {
    expect(placeValue(0)).toBe(1e9);
    expect(placeValue(3)).toBe(1e6);
    expect(placeValue(6)).toBe(1e3);
    expect(placeValue(9)).toBe(1);
    expect(placeLabel(2)).toBe("10 MHz");
    expect(placeLabel(4)).toBe("100 kHz");
    expect(placeLabel(9)).toBe("1 Hz");
  });

  it("splits a frequency into ten digits", () => {
    expect(digitsOf(145_500_000)).toEqual([0, 1, 4, 5, 5, 0, 0, 0, 0, 0]);
    expect(digitsOf(-5)).toEqual([0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
  });

  it("steps the chosen digit with carry", () => {
    expect(stepDigit(145_500_000, 6, 1)).toBe(145_501_000);
    expect(stepDigit(145_999_000, 6, 1)).toBe(146_000_000);
    expect(stepDigit(100_000_000, 2, -1)).toBe(90_000_000);
    expect(stepDigit(433_920_000, 9, -1)).toBe(433_919_999);
  });

  it("clamps to the allowed range", () => {
    expect(stepDigit(5, 9, -1)).toBe(4);
    expect(stepDigit(1, 9, -1)).toBe(1);
    expect(stepDigit(500, 9, -1, 500)).toBe(500);
    expect(stepDigit(1_700_000_000, 3, 1, 1, 1_702_000_000)).toBe(1_701_000_000);
    expect(stepDigit(1_701_500_000, 2, 1, 1, 1_702_000_000)).toBe(1_702_000_000);
    expect(stepDigit(DEFAULT_MAX_HZ, 9, 1)).toBe(DEFAULT_MAX_HZ);
  });

  it("overwrites a single digit", () => {
    expect(setDigit(145_500_000, 5, 7)).toBe(145_570_000);
    expect(setDigit(145_500_000, 3, 0)).toBe(140_500_000);
  });

  it("counts dimmed leading zeros but keeps the last MHz digit", () => {
    expect(leadingZeros(145_500_000)).toBe(1);
    expect(leadingZeros(5_000_000)).toBe(3);
    expect(leadingZeros(500)).toBe(3);
    expect(leadingZeros(1_234_000_000)).toBe(0);
  });

  it("normalises wheel delta modes", () => {
    expect(wheelPixels({ deltaY: 100, deltaMode: 0 })).toBe(100);
    expect(wheelPixels({ deltaY: 3, deltaMode: 1 })).toBe(99);
    expect(wheelPixels({ deltaY: 1, deltaMode: 2 })).toBe(400);
  });
});
