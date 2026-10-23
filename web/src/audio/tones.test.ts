import { describe, expect, it } from "vitest";
import {
  CTCSS_TONES,
  MAX_DIGITS,
  applyTones,
  isEmptyTones,
  parseTones,
  sameTones,
  type TonesMessage,
} from "./tones";

const msg = (over: Partial<TonesMessage> = {}): TonesMessage => ({
  vfo: 1,
  ctcssHz: null,
  dtmfCount: 0,
  dtmfTail: "",
  ...over,
});

describe("parseTones", () => {
  it("reads a message", () => {
    expect(
      parseTones({ v: 1, type: "tones", vfo: 3, ctcss_hz: 88.5, dtmf_count: 4, dtmf_tail: "12*A" }),
    ).toEqual({ vfo: 3, ctcssHz: 88.5, dtmfCount: 4, dtmfTail: "12*A" });
  });

  it("treats a missing tone as none and rejects other messages", () => {
    const m = parseTones({ v: 1, type: "tones", vfo: 1, ctcss_hz: null, dtmf_count: 0 });
    expect(m && isEmptyTones(m)).toBe(true);
    expect(parseTones({ v: 1, type: "rds", vfo: 1 })).toBeNull();
    expect(parseTones({ v: 2, type: "tones", vfo: 1, dtmf_count: 0 })).toBeNull();
    expect(parseTones({ v: 1, type: "tones", vfo: 0, dtmf_count: 0 })).toBeNull();
    expect(parseTones({ v: 1, type: "tones", vfo: 1, dtmf_count: -1 })).toBeNull();
    expect(parseTones({ v: 1, type: "tones", vfo: 1, dtmf_count: "1" })).toBeNull();
    expect(parseTones(null)).toBeNull();
  });

  it("drops a tail with foreign characters", () => {
    expect(
      parseTones({ v: 1, type: "tones", vfo: 1, dtmf_count: 2, dtmf_tail: "1x" })?.dtmfTail,
    ).toBe("");
  });
});

describe("applyTones", () => {
  it("shows the tail of the first message and appends only new digits afterwards", () => {
    let s = applyTones(undefined, msg({ dtmfCount: 3, dtmfTail: "123" }));
    expect(s.digits).toBe("123");
    s = applyTones(s, msg({ dtmfCount: 3, dtmfTail: "123" }));
    expect(s.digits).toBe("123");
    s = applyTones(s, msg({ dtmfCount: 5, dtmfTail: "12345" }));
    expect(s.digits).toBe("12345");
  });

  it("survives a lost message (several new digits at once)", () => {
    let s = applyTones(undefined, msg({ dtmfCount: 1, dtmfTail: "1" }));
    s = applyTones(s, msg({ dtmfCount: 4, dtmfTail: "1234" }));
    expect(s.digits).toBe("1234");
  });

  it("does not bring back digits the user cleared", () => {
    let s = applyTones(undefined, msg({ dtmfCount: 2, dtmfTail: "42" }));
    s = { ...s, digits: "" };
    s = applyTones(s, msg({ dtmfCount: 2, dtmfTail: "42", ctcssHz: 100 }));
    expect(s.digits).toBe("");
    expect(s.ctcssHz).toBe(100);
    s = applyTones(s, msg({ dtmfCount: 3, dtmfTail: "421" }));
    expect(s.digits).toBe("1");
  });

  it("restarts when the counter goes backwards", () => {
    let s = applyTones(undefined, msg({ dtmfCount: 9, dtmfTail: "123456789" }));
    s = applyTones(s, msg({ dtmfCount: 1, dtmfTail: "7" }));
    expect(s.digits).toBe("7");
    expect(s.count).toBe(1);
  });

  it("keeps a bounded string", () => {
    let s = applyTones(undefined, msg());
    for (let i = 1; i <= MAX_DIGITS + 10; i++) {
      s = applyTones(s, msg({ dtmfCount: i, dtmfTail: "5" }));
    }
    expect(s.digits).toHaveLength(MAX_DIGITS);
  });

  it("compares states", () => {
    const a = applyTones(undefined, msg({ ctcssHz: 88.5 }));
    expect(sameTones(a, { ...a })).toBe(true);
    expect(sameTones(a, { ...a, ctcssHz: 91.5 })).toBe(false);
  });
});

describe("CTCSS_TONES", () => {
  it("lists the 50 standard tones in ascending order", () => {
    expect(CTCSS_TONES).toHaveLength(50);
    expect([...CTCSS_TONES].sort((a, b) => a - b)).toEqual([...CTCSS_TONES]);
    expect(CTCSS_TONES[0]).toBe(67.0);
    expect(CTCSS_TONES[49]).toBe(254.1);
  });
});
