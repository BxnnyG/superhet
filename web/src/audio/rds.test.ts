import { describe, expect, it } from "vitest";
import {
  isEmptyRds,
  parseRds,
  ptyName,
  rdsQualityLevel,
  rdsTooWeakForText,
  sameRds,
  type RdsState,
} from "./rds";

const full = {
  v: 1,
  type: "rds",
  vfo: 2,
  synced: true,
  pi: "d3c2",
  pty: 10,
  tp: true,
  ta: false,
  ms: true,
  ps: "SUPERHET",
  rt: "Hello",
};

describe("rds quality", () => {
  it("reads and range-checks bler and quality", () => {
    expect(parseRds({ ...full, bler: 12, quality: 88 })?.state).toMatchObject({
      bler: 12,
      quality: 88,
    });
    expect(parseRds(full)?.state).toMatchObject({ bler: null, quality: null });
    expect(parseRds({ ...full, bler: 101, quality: "x" })?.state).toMatchObject({
      bler: null,
      quality: null,
    });
  });

  it("grades reception and spots signals too weak for text", () => {
    const s = parseRds(full)?.state as RdsState;
    expect(rdsQualityLevel({ synced: false, bler: 0 })).toBe("none");
    expect(rdsQualityLevel({ synced: true, bler: null })).toBe("fair");
    expect(rdsQualityLevel({ synced: true, bler: 10 })).toBe("good");
    expect(rdsQualityLevel({ synced: true, bler: 35 })).toBe("fair");
    expect(rdsQualityLevel({ synced: true, bler: 36 })).toBe("poor");
    expect(rdsTooWeakForText({ ...s, bler: 60, ps: null })).toBe(true);
    expect(rdsTooWeakForText({ ...s, bler: 60 })).toBe(false);
    expect(rdsTooWeakForText({ ...s, bler: 20, ps: null })).toBe(false);
    expect(rdsTooWeakForText({ ...s, synced: false, bler: 60, ps: null })).toBe(false);
  });

  it("keeps a quality-only state on screen and treats a changed rate as a change", () => {
    const empty = parseRds({ v: 1, type: "rds", vfo: 1, synced: false })?.state as RdsState;
    expect(isEmptyRds(empty)).toBe(true);
    expect(isEmptyRds({ ...empty, quality: 0 })).toBe(false);
    expect(sameRds({ ...empty, bler: 3, quality: 97 }, { ...empty, bler: 4, quality: 96 })).toBe(
      false,
    );
  });
});

describe("parseRds", () => {
  it("reads a full message", () => {
    const m = parseRds(full);
    expect(m?.vfo).toBe(2);
    expect(m?.state).toEqual({
      synced: true,
      pi: "D3C2",
      pty: 10,
      tp: true,
      ta: false,
      ms: true,
      ps: "SUPERHET",
      rt: "Hello",
      bler: null,
      quality: null,
    });
  });

  it("treats missing or wrong-typed fields as unknown", () => {
    const m = parseRds({ v: 1, type: "rds", vfo: 1, synced: false, pty: 99, pi: "xyz", ps: 5 });
    expect(m?.state.pty).toBeNull();
    expect(m?.state.pi).toBeNull();
    expect(m?.state.ps).toBeNull();
    expect(m && isEmptyRds(m.state)).toBe(true);
  });

  it("rejects other messages", () => {
    expect(parseRds(null)).toBeNull();
    expect(parseRds({ ...full, type: "ack" })).toBeNull();
    expect(parseRds({ ...full, v: 2 })).toBeNull();
    expect(parseRds({ ...full, vfo: 0 })).toBeNull();
    expect(parseRds({ ...full, vfo: "1" })).toBeNull();
  });

  it("compares states", () => {
    const a = parseRds(full)?.state;
    const b = parseRds({ ...full, rt: "Other" })?.state;
    expect(a && sameRds(a, a)).toBe(true);
    expect(a && b && sameRds(a, b)).toBe(false);
  });
});

describe("ptyName", () => {
  it("uses the European table unless the de-emphasis says North America", () => {
    expect(ptyName(10, "50us")).toBe("Pop music");
    expect(ptyName(10, null)).toBe("Pop music");
    expect(ptyName(10, "75us")).toBe("Country");
    expect(ptyName(31, "50us")).toBe("Alarm");
    expect(ptyName(40, "50us")).toBe("PTY 40");
  });
});
