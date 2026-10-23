import { describe, expect, it } from "vitest";
import { isEmptyRds, parseRds, ptyName, sameRds } from "./rds";

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
