import { describe, expect, it } from "vitest";
import { bestScore, fuzzyScore, rank, rankScored, termScore } from "./fuzzy";

describe("termScore", () => {
  it("matches substrings and subsequences, case-insensitively", () => {
    expect(termScore("ntf", "NFM Tetra Funk")).not.toBeNull();
    expect(termScore("TETRA", "tetra funk")).not.toBeNull();
    expect(termScore("xyz", "tetra funk")).toBeNull();
    expect(termScore("ba", "ab")).toBeNull(); // order matters
  });

  it("ranks prefix over word start over inside over scattered", () => {
    const prefix = termScore("air", "Airband Tower");
    const word = termScore("air", "Munich Airport");
    const inside = termScore("air", "Repair shop");
    const scattered = termScore("arb", "Airband Tower");
    expect(prefix).toBeGreaterThan(word ?? 0);
    expect(word).toBeGreaterThan(inside ?? 0);
    expect(inside).toBeGreaterThan(scattered ?? 0);
  });

  it("rewards contiguous runs in subsequence matches", () => {
    expect(termScore("abc", "a-b-c")).toBeLessThan(termScore("abc", "xabcx") ?? 0);
  });
});

describe("fuzzyScore", () => {
  it("requires every term, in any order", () => {
    expect(fuzzyScore("tower munich", "Munich Tower 118.1")).not.toBeNull();
    expect(fuzzyScore("tower paris", "Munich Tower 118.1")).toBeNull();
  });

  it("treats an empty query as a match of everything", () => {
    expect(fuzzyScore("   ", "anything")).toBe(0);
  });

  it("prefers the shorter text for the same match", () => {
    expect(fuzzyScore("tower", "Tower")).toBeGreaterThan(
      fuzzyScore("tower", "Tower control east") ?? 0,
    );
  });
});

describe("bestScore", () => {
  it("takes the best text but weights the first (title) highest", () => {
    const viaTitle = bestScore("rds", ["RDS decoder", "keyword"]);
    const viaKeyword = bestScore("rds", ["Decoder log", "rds events"]);
    expect(viaTitle).toBeGreaterThan(viaKeyword ?? 0);
    expect(bestScore("zzz", ["a", "b"])).toBeNull();
  });
});

describe("rank", () => {
  const items = [
    { t: "New VFO at the display centre" },
    { t: "Close the active VFO" },
    { t: "Switch mode to USB" },
    { t: "Export the decoder log (CSV)" },
  ];

  it("drops non-matches and puts the best match first", () => {
    const out = rank(items, "vfo", (i) => [i.t]).map((i) => i.t);
    expect(out).toEqual(["New VFO at the display centre", "Close the active VFO"]);
    expect(rank(items, "usb", (i) => [i.t])[0]?.t).toBe("Switch mode to USB");
    expect(rank(items, "log csv", (i) => [i.t])[0]?.t).toBe("Export the decoder log (CSV)");
  });

  it("keeps input order for an empty query and for equal scores", () => {
    expect(rank(items, "", (i) => [i.t])).toEqual(items);
    const same = [{ t: "alpha" }, { t: "alpha" }];
    expect(rank(same, "alp", (i) => [i.t])).toEqual(same);
  });

  it("returns scores with rankScored", () => {
    const out = rankScored(items, "vfo", (i) => [i.t]);
    expect(out).toHaveLength(2);
    expect(out[0]?.score).toBeGreaterThanOrEqual(out[1]?.score ?? 0);
  });
});
