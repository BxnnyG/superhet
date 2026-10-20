import { describe, expect, it } from "vitest";
import { formatFrequency, parseFrequency } from "./freq";

describe("parseFrequency", () => {
  it.each([
    ["104.2", 104_200_000],
    ["104,2", 104_200_000],
    ["433.92M", 433_920_000],
    ["433.92 MHz", 433_920_000],
    ["7100k", 7_100_000],
    ["7100 kHz", 7_100_000],
    ["145500000", 145_500_000],
    ["2.4G", 2_400_000_000],
    ["  1296.5m ", 1_296_500_000],
    ["433.920 000", 433_920_000],
    ["12345hz", 12_345],
  ])("parses %s", (input, hz) => {
    expect(parseFrequency(input)).toBe(hz);
  });

  it.each(["", "abc", "1.2.3", "-5", "0", "12x", "M", "1e6"])("rejects %j", (input) => {
    expect(parseFrequency(input)).toBeNull();
  });
});

describe("formatFrequency", () => {
  it("groups MHz, kHz and Hz", () => {
    expect(formatFrequency(433_920_000)).toBe("433.920 000");
    expect(formatFrequency(7_100_001)).toBe("7.100 001");
    expect(formatFrequency(104_200_000)).toBe("104.200 000");
  });

  it("round-trips through the parser", () => {
    expect(parseFrequency(formatFrequency(145_512_500))).toBe(145_512_500);
  });
});
